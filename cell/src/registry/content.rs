//! An org's content governance in the registry (docs/cloudflare-v1.md,
//! decisions 69 to 71; the rules are `fragment_core::governance`): who of
//! its people are in its content (`org_content`, at an epoch), its content
//! admins, the admins' places offered to an email (`org_content_offers`),
//! and its governance log (`org_governance`), apart from its billing's
//! `org_members`: a seat or a billing admin's place gives none of it.
//!
//! Every change is a person's own (an agent acts for no one here), or an
//! operator's recovery of an org with no content admin, and each is
//! logged under its request id in the same turn as its write: a replay
//! finds its first and answers as it did. An offboarding raises the
//! person's epoch and makes them inactive at once, before anything of
//! theirs is cleaned up elsewhere.

use fragment_core::governance::{self, Answer, Offerer, Place, Refusal, LOG_PAGE};
use fragment_proto::org::{ContentOffer, ContentOffers, ContentPerson, GovernanceEntry, OrgContent};
use serde::Serialize;

use super::calls::{By, Call};
use super::orgs::{fresh_id, not_found, OrgRow};
use super::*;

/// `POST /content/offer`: a content admin's place offered to `email`, by
/// `by` (an admin; who pays, while the org has none), or by `operator`
/// (the router checked them; the recovery of an org with none).
#[derive(Serialize, Deserialize)]
pub(crate) struct OfferAdmin {
    pub by: Option<By>,
    pub operator: Option<String>,
    pub org: String,
    pub email: String,
    pub request: String,
}

impl Call for OfferAdmin {
    const PATH: &'static str = "/content/offer";
    type Answer = ContentOffer;
}

/// `POST /content/answer`: the asker answers an offer to an email of theirs.
#[derive(Serialize, Deserialize)]
pub(crate) struct AnswerOffer {
    pub by: By,
    pub org: String,
    pub offer: String,
    pub answer: Answer,
}

impl Call for AnswerOffer {
    const PATH: &'static str = "/content/answer";
    type Answer = ContentOffer;
}

/// `POST /content/offers`: the offers waiting on the asker's emails.
#[derive(Serialize, Deserialize)]
pub(crate) struct OffersToMe {
    pub by: By,
}

impl Call for OffersToMe {
    const PATH: &'static str = "/content/offers";
    type Answer = ContentOffers;
}

/// `POST /content/step-down`: an admin no more (the asker, or another
/// admin they name); still in the org's content. Never its last.
#[derive(Serialize, Deserialize)]
pub(crate) struct StepDown {
    pub by: By,
    pub org: String,
    pub person: String,
}

impl Call for StepDown {
    const PATH: &'static str = "/content/step-down";
    type Answer = ContentPerson;
}

/// `POST /content/offboard`: `person` out of the org's content at once.
#[derive(Serialize, Deserialize)]
pub(crate) struct OffboardCall {
    pub by: By,
    pub org: String,
    pub person: String,
    pub expected_epoch: u64,
    pub request: String,
}

impl Call for OffboardCall {
    const PATH: &'static str = "/content/offboard";
    type Answer = ContentPerson;
}

/// `POST /content/view`: an org's content, to its content admins or an
/// operator (`by` none: the router checked them), its log before `before`.
#[derive(Serialize, Deserialize)]
pub(crate) struct ContentOf {
    pub by: Option<By>,
    pub org: String,
    pub before: Option<u64>,
}

impl Call for ContentOf {
    const PATH: &'static str = "/content/view";
    type Answer = OrgContent;
}

#[derive(Deserialize)]
struct PlaceRow {
    epoch: u64,
    active: i64,
    admin: i64,
}

impl PlaceRow {
    fn place(&self) -> Place {
        Place { epoch: self.epoch, active: self.active != 0, admin: self.admin != 0 }
    }
}

#[derive(Deserialize)]
struct OfferRow {
    id: String,
    org: String,
    email: String,
    offered_by: String,
    offered_at: i64,
    answer: Option<String>,
}

const OFFER_COLUMNS: &str = "id, org, email, offered_by, offered_at, answer";

fn refused(r: Refusal) -> CellError {
    CellError::new(r.code(), r.message())
}

fn request_arg(id: &str) -> CellResult<()> {
    match governance::valid_request(id) {
        true => Ok(()),
        false => Err(CellError::invalid(format!("an id is 1-{} printable ASCII characters, no spaces", governance::REQUEST_MAX_BYTES))),
    }
}

impl RegistryCell {
    /// The org named, or 404.
    fn content_org(&self, org: &str) -> CellResult<OrgRow> {
        match fragment_core::org::valid_org_id(org) {
            true => self.org_row(org)?.ok_or_else(|| not_found(format!("no org {org}"))),
            false => Err(not_found(format!("no org {org}"))),
        }
    }

    /// Who asks, a person acting as themselves.
    fn governor(&self, by: &By) -> CellResult<Identity> {
        let who = self.by(by)?;
        if who.kind != IdentityKind::Person {
            return Err(CellError::new(ErrorCode::Forbidden, "an org's content is governed by people, each as themselves: an agent acts for no one here"));
        }
        Ok(who)
    }

    fn place_in(&self, org: &str, person: &str) -> CellResult<Option<Place>> {
        Ok(self.row::<PlaceRow>("SELECT epoch, active, admin FROM org_content WHERE org = ? AND person = ?", vec![org.into(), person.into()])?.map(|r| r.place()))
    }

    /// The org's active content admins.
    fn admins_in(&self, org: &str) -> CellResult<u64> {
        self.count("SELECT COUNT(*) AS n FROM org_content WHERE org = ? AND admin = 1", vec![org.into()])
    }

    /// The admin `who` is of `org`, or 403.
    fn content_admin(&self, org: &str, who: &str) -> CellResult<Place> {
        match self.place_in(org, who)? {
            Some(p) if p.admin => Ok(p),
            _ => Err(refused(Refusal::NotAdmin)),
        }
    }

    fn write_place(&self, org: &str, person: &str, p: Place) -> CellResult<()> {
        self.exec(
            "INSERT INTO org_content (org, person, epoch, active, admin, since) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (org, person) DO UPDATE SET epoch = excluded.epoch, active = excluded.active, admin = excluded.admin,
               since = CASE WHEN org_content.active = 0 AND excluded.active = 1 THEN excluded.since ELSE org_content.since END,
               left_at = CASE WHEN org_content.active = 1 AND excluded.active = 0 THEN excluded.since ELSE org_content.left_at END",
            vec![
                org.into(),
                person.into(),
                SqlStorageValue::Integer(p.epoch as i64),
                SqlStorageValue::Integer(i64::from(p.active)),
                SqlStorageValue::Integer(i64::from(p.admin)),
                SqlStorageValue::Integer(js::now_ms()),
            ],
        )
    }

    /// An entry of `org`'s governance log, in the caller's turn.
    fn govern(&self, org: &str, request: &str, actor: &str, action: &str, subject: Option<&str>, detail: Value) -> CellResult<()> {
        self.exec(
            "INSERT INTO org_governance (org, request, at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?, ?, ?)",
            vec![
                org.into(),
                request.into(),
                SqlStorageValue::Integer(js::now_ms()),
                actor.into(),
                action.into(),
                subject.map_or(SqlStorageValue::Null, |s| s.into()),
                detail.to_string().into(),
            ],
        )
    }

    /// The entry a request id made in `org`'s log, if any.
    fn governed(&self, org: &str, request: &str) -> CellResult<Option<GovernanceEntry>> {
        self.row::<GovernanceEntry>("SELECT seq, at, actor, action, subject, request, detail FROM org_governance WHERE org = ? AND request = ?", vec![org.into(), request.into()])
    }

    /// The latest email `person` signed in with.
    fn email_now(&self, person: &str) -> CellResult<Option<String>> {
        #[derive(Deserialize)]
        struct Row {
            email: String,
        }
        Ok(self.row::<Row>("SELECT email FROM subjects WHERE identity = ? ORDER BY signed_in_at DESC LIMIT 1", vec![person.into()])?.map(|r| r.email))
    }

    fn offer_view(&self, r: OfferRow) -> CellResult<ContentOffer> {
        let org = self.content_org(&r.org)?.reference();
        let from = if npub::is_identity(&r.offered_by) { self.email_now(&r.offered_by)? } else { None };
        Ok(ContentOffer { id: r.id, org, email: r.email, from, at: r.offered_at, answer: r.answer })
    }

    fn offer_row(&self, org: &str, id: &str) -> CellResult<OfferRow> {
        self.row::<OfferRow>(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE id = ? AND org = ?"), vec![id.into(), org.into()])?
            .ok_or_else(|| not_found(format!("no offer {id}")))
    }

    fn offer_admin(&self, b: OfferAdmin) -> CellResult<ContentOffer> {
        request_arg(&b.request)?;
        let org = self.content_org(&b.org)?;
        let email = super::signin::email_of(&b.email).ok_or_else(|| CellError::invalid(format!("{:?} is not an email", b.email)))?;
        let (actor, offerer) = match (&b.by, &b.operator) {
            (None, Some(op)) => (op.clone(), Offerer::Operator),
            (Some(by), None) => {
                let who = self.governor(by)?;
                let offerer = match (self.place_in(&org.id, &who.id)?, self.payer_of(&org.id)?) {
                    (Some(p), _) if p.admin => Offerer::Admin,
                    (_, Some(payer)) if payer == who.id => Offerer::Payer,
                    _ => Offerer::Other,
                };
                (who.id, offerer)
            }
            _ => return Err(CellError::host("an offer is a person's or an operator's")),
        };
        // a replay: the offer its request made, the same email's
        if let Some(r) = self.row::<OfferRow>(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE org = ? AND request = ?"), vec![org.id.as_str().into(), b.request.as_str().into()])? {
            return match r.email == email && r.offered_by == actor {
                true => self.offer_view(r),
                false => Err(CellError::new(ErrorCode::ConflictingBody, "this id was used for another offer")),
            };
        }
        let waiting = self.count("SELECT COUNT(*) AS n FROM org_content_offers WHERE org = ? AND answer IS NULL", vec![org.id.as_str().into()])?;
        let people = self.count("SELECT COUNT(*) AS n FROM org_content WHERE org = ?", vec![org.id.as_str().into()])?;
        governance::may_offer(offerer, people, self.admins_in(&org.id)?, waiting).map_err(refused)?;
        // offered and waiting already: that one
        if let Some(r) = self.row::<OfferRow>(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE org = ? AND email = ? AND answer IS NULL"), vec![org.id.as_str().into(), email.as_str().into()])? {
            return self.offer_view(r);
        }
        let id = fresh_id("cof-");
        let now = js::now_ms();
        self.exec(
            "INSERT INTO org_content_offers (id, org, email, request, offered_by, offered_at) VALUES (?, ?, ?, ?, ?, ?)",
            vec![id.as_str().into(), org.id.as_str().into(), email.as_str().into(), b.request.as_str().into(), actor.as_str().into(), SqlStorageValue::Integer(now)],
        )?;
        let action = match offerer {
            Offerer::Operator => "admin-offered-by-operator",
            Offerer::Payer => "first-admin-offered",
            _ => "admin-offered",
        };
        self.govern(&org.id, &b.request, &actor, action, Some(&email), json!({ "offer": id }))?;
        self.offer_view(self.offer_row(&org.id, &id)?)
    }

    fn answer_offer(&self, b: AnswerOffer) -> CellResult<ContentOffer> {
        let who = self.governor(&b.by)?;
        let org = self.content_org(&b.org)?;
        let offer = self.offer_row(&org.id, &b.offer)?;
        // only whoever signs in with its email sees it
        let theirs = self.count("SELECT COUNT(*) AS n FROM subjects WHERE identity = ? AND email = ?", vec![who.id.as_str().into(), offer.email.as_str().into()])? > 0;
        if !theirs {
            return Err(not_found(format!("no offer {}", b.offer)));
        }
        let recorded = offer.answer.as_deref().map(|a| Answer::parse(a).ok_or_else(|| CellError::host(format!("the offer {}'s answer is {a:?}", offer.id)))).transpose()?;
        if !governance::answer(recorded, b.answer).map_err(refused)? {
            return self.offer_view(offer);
        }
        if b.answer == Answer::Accepted {
            let place = governance::accepted(self.place_in(&org.id, &who.id)?);
            self.write_place(&org.id, &who.id, place)?;
        }
        let now = js::now_ms();
        self.exec(
            "UPDATE org_content_offers SET answer = ?, answered_by = ?, answered_at = ? WHERE id = ? AND answer IS NULL",
            vec![b.answer.as_str().into(), who.id.as_str().into(), SqlStorageValue::Integer(now), offer.id.as_str().into()],
        )?;
        let action = format!("admin-{}", b.answer.as_str());
        self.govern(&org.id, &format!("answer:{}", offer.id), &who.id, &action, Some(&who.id), json!({ "offer": offer.id, "email": offer.email }))?;
        self.offer_view(self.offer_row(&org.id, &offer.id)?)
    }

    fn offers_to_me(&self, b: OffersToMe) -> CellResult<ContentOffers> {
        let who = self.governor(&b.by)?;
        // bounded: at most 8 emails a person, each offered once an org
        let rows = self.rows::<OfferRow>(
            &format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE answer IS NULL AND email IN (SELECT email FROM subjects WHERE identity = ?) ORDER BY offered_at, id LIMIT 100"),
            vec![who.id.as_str().into()],
        )?;
        Ok(ContentOffers { offers: rows.into_iter().map(|r| self.offer_view(r)).collect::<CellResult<_>>()? })
    }

    fn person_view(&self, org: &str, person: &str) -> CellResult<ContentPerson> {
        #[derive(Deserialize)]
        struct Row {
            epoch: u64,
            active: i64,
            admin: i64,
            since: i64,
        }
        let r = self
            .row::<Row>("SELECT epoch, active, admin, since FROM org_content WHERE org = ? AND person = ?", vec![org.into(), person.into()])?
            .ok_or_else(|| not_found(format!("{person} is not in the org's content")))?;
        Ok(ContentPerson { person: person.to_string(), email: self.email_now(person)?, epoch: r.epoch, active: r.active != 0, admin: r.admin != 0, since: r.since })
    }

    fn step_down(&self, b: StepDown) -> CellResult<ContentPerson> {
        let who = self.governor(&b.by)?;
        let org = self.content_org(&b.org)?;
        self.content_admin(&org.id, &who.id)?;
        let place = self.place_in(&org.id, &b.person)?.ok_or_else(|| refused(Refusal::NotIn))?;
        let next = governance::removed_admin(place, self.admins_in(&org.id)?).map_err(refused)?;
        if next != place {
            self.write_place(&org.id, &b.person, next)?;
            let request = format!("step-down:{}", hex::encode(js::random_bytes::<8>()));
            self.govern(&org.id, &request, &who.id, "admin-removed", Some(&b.person), json!({ "epoch": next.epoch }))?;
        }
        self.person_view(&org.id, &b.person)
    }

    fn offboard(&self, b: OffboardCall) -> CellResult<ContentPerson> {
        request_arg(&b.request)?;
        let who = self.governor(&b.by)?;
        let org = self.content_org(&b.org)?;
        self.content_admin(&org.id, &who.id)?;
        if let Some(e) = self.governed(&org.id, &b.request)? {
            return match e.action == "offboarded" && e.subject.as_deref() == Some(b.person.as_str()) && e.actor == who.id {
                true => self.person_view(&org.id, &b.person),
                false => Err(CellError::new(ErrorCode::ConflictingBody, "this id was used for another change")),
            };
        }
        let place = self.place_in(&org.id, &b.person)?.ok_or_else(|| refused(Refusal::NotIn))?;
        let next = governance::offboarded(place, b.expected_epoch, self.admins_in(&org.id)?).map_err(refused)?;
        self.write_place(&org.id, &b.person, next)?;
        self.govern(&org.id, &b.request, &who.id, "offboarded", Some(&b.person), json!({ "from": place.epoch, "to": next.epoch }))?;
        console_log!("{}", json!({ "event": "org.offboarded", "org": org.id, "person": b.person, "epoch": next.epoch }));
        self.person_view(&org.id, &b.person)
    }

    fn content_of(&self, b: ContentOf) -> CellResult<OrgContent> {
        let org = self.content_org(&b.org)?;
        if let Some(by) = &b.by {
            let who = self.governor(by)?;
            self.content_admin(&org.id, &who.id)?;
        }
        #[derive(Deserialize)]
        struct Person {
            person: String,
        }
        // bounded: the people who said yes to it, and its admins' offers
        let people = self.rows::<Person>("SELECT person FROM org_content WHERE org = ? ORDER BY since, person LIMIT 1000", vec![org.id.as_str().into()])?;
        let people = people.iter().map(|p| self.person_view(&org.id, &p.person)).collect::<CellResult<Vec<_>>>()?;
        let offers = self.rows::<OfferRow>(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE org = ? AND answer IS NULL ORDER BY offered_at, id"), vec![org.id.as_str().into()])?;
        let offers = offers.into_iter().map(|r| self.offer_view(r)).collect::<CellResult<Vec<_>>>()?;
        let before = b.before.unwrap_or(i64::MAX as u64).min(i64::MAX as u64) as i64;
        let log = self.rows::<GovernanceEntry>(
            "SELECT seq, at, actor, action, subject, request, detail FROM org_governance WHERE org = ? AND seq < ? ORDER BY seq DESC LIMIT ?",
            vec![org.id.as_str().into(), SqlStorageValue::Integer(before), SqlStorageValue::Integer(LOG_PAGE as i64)],
        )?;
        let next = (log.len() as u64 == LOG_PAGE).then(|| log.last().map(|e| e.seq)).flatten();
        Ok(OrgContent { org: org.reference(), people, offers, log, next })
    }

    /// Whether `org` has anything of its content: its ledger and estate
    /// outlive its billing (decision 69), so it is never dropped with it.
    pub(super) fn has_content(&self, org: &str) -> CellResult<bool> {
        Ok(self.count("SELECT COUNT(*) AS n FROM org_content WHERE org = ?", vec![org.into()])? > 0)
    }

    /// A wiped person's places in every org's content: out at once, at
    /// their next epoch, logged as the operator's (the org's last admin
    /// too: its estate waits on an operator's recovery). Their offers'
    /// answers and the log's entries naming them stay.
    pub(super) fn content_wiped(&self, person: &str, operator: &str) -> CellResult<()> {
        #[derive(Deserialize)]
        struct Row {
            org: String,
            epoch: u64,
        }
        // bounded: the orgs whose content they said yes to
        for r in self.rows::<Row>("SELECT org, epoch FROM org_content WHERE person = ? AND active = 1", vec![person.into()])? {
            let next = Place { epoch: r.epoch + 1, active: false, admin: false };
            self.write_place(&r.org, person, next)?;
            self.govern(&r.org, &format!("wipe:{person}:{}", r.epoch), operator, "wiped", Some(person), json!({ "from": r.epoch, "to": next.epoch }))?;
        }
        Ok(())
    }

    pub(super) fn content_route(&self, path: &str, bytes: &[u8]) -> Option<CellResult<Response>> {
        Some(match path {
            OfferAdmin::PATH => reply::<OfferAdmin>(body(bytes).and_then(|b| self.offer_admin(b))),
            AnswerOffer::PATH => reply::<AnswerOffer>(body(bytes).and_then(|b| self.answer_offer(b))),
            OffersToMe::PATH => reply::<OffersToMe>(body(bytes).and_then(|b| self.offers_to_me(b))),
            StepDown::PATH => reply::<StepDown>(body(bytes).and_then(|b| self.step_down(b))),
            OffboardCall::PATH => reply::<OffboardCall>(body(bytes).and_then(|b| self.offboard(b))),
            ContentOf::PATH => reply::<ContentOf>(body(bytes).and_then(|b| self.content_of(b))),
            _ => return None,
        })
    }
}
