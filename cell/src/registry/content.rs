//! An org's content governance in the registry (docs/cloudflare-v1.md,
//! decisions 69 to 71; the rules are `fragment_core::governance`): its
//! content admins (`org_content`), the admins' places offered to an email
//! (`org_content_offers`), and its governance log (`org_governance`), apart
//! from its billing's `org_members`: a seat or a billing admin's place
//! gives none of it.
//!
//! Every change is a person's own (an agent acts for no one here), or an
//! operator's recovery of an org with no content admin. Each runs in one
//! `transactionSync` with its log entry (`atomically`): a statement that
//! fails rolls back the whole change, entry and all (the registry otherwise
//! keeps each statement as it runs). A client's request id is kept
//! prefixed by its kind (`offer:<id>`), apart from the platform's own
//! (`answer:`, `withdraw:`, `step-down:`, `wipe:`), and a replay is found
//! by its entry before anything is written. An offer is accepted only
//! while the grounds it was made on hold, and a person stepping down (or
//! wiped) takes their waiting offers with them, made or received.

use fragment_core::governance::{self, Answer, Grounds, Now, Place, Refusal, Standing, LOG_PAGE, OFFER_TTL_MS};
use fragment_proto::org::{ContentOffer, ContentOffers, ContentPerson, GovernanceEntry, OrgContent, PayerNamed};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::calls::{By, Call};
use super::orgs::{fresh_id, not_found, OrgRow};
use super::*;

/// `POST /content/offer`: a content admin's place offered to `email`, by
/// `by` (an admin; who pays, for the first), or by `operator` (the router
/// checked them; the recovery of an org with none).
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

/// `POST /content/withdraw`: one of the org's admins takes back an offer
/// waiting.
#[derive(Serialize, Deserialize)]
pub(crate) struct WithdrawOffer {
    pub by: By,
    pub org: String,
    pub offer: String,
}

impl Call for WithdrawOffer {
    const PATH: &'static str = "/content/withdraw";
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

/// `POST /content/enroll`: `person` comes into `org`'s content as they
/// come into one of its fragments (their yes to an invite of `invited_at`,
/// or a content admin's own add): their epoch there, the fragment's to
/// bind their membership to. Platform code's alone (a fragment's). An
/// invite made before they last left admits them no more.
#[derive(Serialize, Deserialize)]
pub(crate) struct Enroll {
    pub org: String,
    pub person: String,
    pub invited_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Enrolled {
    pub epoch: u64,
}

impl Call for Enroll {
    const PATH: &'static str = "/content/enroll";
    type Answer = Enrolled;
}

/// `POST /content/payer`: an operator (the router checked them) names the
/// person an org's credit is invoiced to: its governance log's record of
/// who pays, an org paying out of band (Paul, 2026-10-30). It gives them no
/// power of its own.
#[derive(Serialize, Deserialize)]
pub(crate) struct NominatePayer {
    pub operator: String,
    pub org: String,
    pub person: String,
    pub request: String,
}

impl Call for NominatePayer {
    const PATH: &'static str = "/content/payer";
    type Answer = PayerNamed;
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

/// One change's statements, inside its `transactionSync` (`atomically`).
/// Each answers a value; a statement that fails is the change's failure.
struct Tx(SqlStorage);

impl Tx {
    fn exec(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<()> {
        self.0.exec(q, binds)?;
        Ok(())
    }

    fn rows<T: DeserializeOwned>(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<Vec<T>> {
        Ok(self.0.exec(q, binds)?.to_array()?)
    }

    fn row<T: DeserializeOwned>(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<Option<T>> {
        Ok(self.rows(q, binds)?.into_iter().next())
    }

    fn count(&self, q: &str, binds: Vec<SqlStorageValue>) -> CellResult<u64> {
        #[derive(Deserialize)]
        struct N {
            n: u64,
        }
        Ok(self.row::<N>(q, binds)?.map_or(0, |r| r.n))
    }

    /// Whether `person` is one of `org`'s content admins now, and whether
    /// they are in its content at all.
    fn place(&self, org: &str, person: &str) -> CellResult<Option<bool>> {
        #[derive(Deserialize)]
        struct Row {
            admin: i64,
        }
        Ok(self.row::<Row>("SELECT admin FROM org_content WHERE org = ? AND person = ?", vec![org.into(), person.into()])?.map(|r| r.admin != 0))
    }

    /// `person`'s place in `org`'s content, and when they last left it.
    fn place_of(&self, org: &str, person: &str) -> CellResult<Option<(Place, Option<i64>)>> {
        #[derive(Deserialize)]
        struct Row {
            epoch: u64,
            active: i64,
            admin: i64,
            left_at: Option<i64>,
        }
        let row = self.row::<Row>("SELECT epoch, active, admin, left_at FROM org_content WHERE org = ? AND person = ?", vec![org.into(), person.into()])?;
        Ok(row.map(|r| (Place { epoch: r.epoch, active: r.active != 0, admin: r.admin != 0 }, r.left_at)))
    }

    /// `person`'s place written, `left_at` now when they leave, and the
    /// bound on the orgs' content they are in at once when they come in.
    fn write_place(&self, org: &str, person: &str, was: Option<Place>, p: Place, now: i64) -> CellResult<()> {
        if p.active && !was.is_some_and(|w| w.active) {
            let places = self.count("SELECT COUNT(*) AS n FROM org_content WHERE person = ? AND active = 1", vec![person.into()])?;
            if places >= governance::ORGS_PER_PERSON_MAX {
                return Err(CellError::invalid(format!("a person is in at most {} orgs' content", governance::ORGS_PER_PERSON_MAX)));
            }
        }
        let left = was.is_some_and(|w| w.active) && !p.active;
        self.exec(
            "INSERT INTO org_content (org, person, epoch, active, admin, since, left_at) VALUES (?, ?, ?, ?, ?, ?, NULL)
             ON CONFLICT (org, person) DO UPDATE SET epoch = excluded.epoch, active = excluded.active, admin = excluded.admin,
               since = CASE WHEN org_content.active = 0 AND excluded.active = 1 THEN excluded.since ELSE org_content.since END,
               left_at = CASE WHEN ? THEN excluded.since ELSE org_content.left_at END",
            vec![
                org.into(),
                person.into(),
                SqlStorageValue::Integer(i64::try_from(p.epoch).expect("an epoch fits SQLite's integers")),
                SqlStorageValue::Integer(i64::from(p.active)),
                SqlStorageValue::Integer(i64::from(p.admin)),
                SqlStorageValue::Integer(now),
                SqlStorageValue::Integer(i64::from(left)),
            ],
        )
    }

    fn admins(&self, org: &str) -> CellResult<u64> {
        self.count("SELECT COUNT(*) AS n FROM org_content WHERE org = ? AND admin = 1", vec![org.into()])
    }

    fn people(&self, org: &str) -> CellResult<u64> {
        self.count("SELECT COUNT(*) AS n FROM org_content WHERE org = ?", vec![org.into()])
    }

    /// `org`'s offers past their time, marked expired.
    fn expire(&self, org: &str, now: i64) -> CellResult<()> {
        self.exec(
            "UPDATE org_content_offers SET answer = 'expired', answered_at = ? WHERE org = ? AND answer IS NULL AND expires_at <= ?",
            vec![SqlStorageValue::Integer(now), org.into(), SqlStorageValue::Integer(now)],
        )
    }

    fn offer(&self, org: &str, id: &str) -> CellResult<OfferRow> {
        self.row::<OfferRow>(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE id = ? AND org = ?"), vec![id.into(), org.into()])?
            .ok_or_else(|| not_found(format!("no offer {id}")))
    }

    /// The waiting offers `person` made, and those to their emails, taken
    /// back as they step down (or are wiped).
    fn retire_offers(&self, org: &str, person: &str, by: &str, now: i64) -> CellResult<Vec<String>> {
        #[derive(Deserialize)]
        struct Id {
            id: String,
        }
        // bounded: an org's waiting offers (ADMINS_MAX, an operator's few)
        let ids = self.rows::<Id>(
            "UPDATE org_content_offers SET answer = 'withdrawn', answered_by = ?, answered_at = ?
             WHERE org = ? AND answer IS NULL AND (offered_by = ? OR email IN (SELECT email FROM subjects WHERE identity = ?)) RETURNING id",
            vec![by.into(), SqlStorageValue::Integer(now), org.into(), person.into(), person.into()],
        )?;
        Ok(ids.into_iter().map(|r| r.id).collect())
    }

    /// `org`'s waiting offers whose grounds hold no more, marked lapsed in
    /// the change that shows it (`governance::lapses`), so none is handed
    /// back as if new, nor counted against the cap. `payer`: who pays now.
    fn settle(&self, org: &str, payer: Option<&str>, now: i64) -> CellResult<Vec<String>> {
        let (people, admins) = (self.people(org)?, self.admins(org)?);
        // bounded: an org's waiting offers (ADMINS_MAX, an operator's few)
        let waiting: Vec<OfferRow> = self.rows(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE org = ? AND answer IS NULL"), vec![org.into()])?;
        let mut lapsed = vec![];
        for o in waiting {
            let grounds = o.grounds()?;
            let offerer_stands = match grounds {
                Grounds::Admin => self.place(org, &o.offered_by)? == Some(true),
                Grounds::Payer => payer == Some(o.offered_by.as_str()),
                Grounds::Operator => true,
            };
            if governance::lapses(grounds, Now { people, admins, offerer_stands }) {
                self.exec(
                    "UPDATE org_content_offers SET answer = 'lapsed', answered_at = ? WHERE id = ? AND answer IS NULL",
                    vec![SqlStorageValue::Integer(now), o.id.as_str().into()],
                )?;
                lapsed.push(o.id);
            }
        }
        Ok(lapsed)
    }

    /// The entry `request` made in `org`'s log, if any.
    fn governed(&self, org: &str, request: &str) -> CellResult<Option<LogRow>> {
        self.row::<LogRow>("SELECT seq, at, actor, action, subject, request, detail FROM org_governance WHERE org = ? AND request = ?", vec![org.into(), request.into()])
    }

    /// An entry of `org`'s governance log.
    fn govern(&self, org: &str, e: Logged<'_>, now: i64) -> CellResult<()> {
        self.exec(
            "INSERT INTO org_governance (org, request, at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?, ?, ?)",
            vec![
                org.into(),
                e.request.into(),
                SqlStorageValue::Integer(now),
                e.actor.into(),
                e.action.into(),
                e.subject.map_or(SqlStorageValue::Null, |s| s.into()),
                e.detail.to_string().into(),
            ],
        )
    }
}

/// A governance log entry to write: under which request, who did it, what,
/// to whom, and its detail (ids, never an email).
struct Logged<'a> {
    request: &'a str,
    actor: &'a str,
    action: &'a str,
    subject: Option<&'a str>,
    detail: Value,
}

#[derive(Deserialize)]
struct OfferRow {
    id: String,
    org: String,
    email: String,
    grounds: String,
    offered_by: String,
    offered_at: i64,
    expires_at: i64,
    answer: Option<String>,
}

impl OfferRow {
    fn standing(&self, now: i64) -> CellResult<Standing> {
        Standing::of(self.answer.as_deref(), self.expires_at, now).ok_or_else(|| CellError::host(format!("the offer {}'s answer is {:?}", self.id, self.answer)))
    }

    fn grounds(&self) -> CellResult<Grounds> {
        Grounds::parse(&self.grounds).ok_or_else(|| CellError::host(format!("the offer {}'s grounds are {:?}", self.id, self.grounds)))
    }
}

const OFFER_COLUMNS: &str = "id, org, email, grounds, offered_by, offered_at, expires_at, answer";

/// A governance log's row: its detail as kept, JSON text.
#[derive(Deserialize)]
struct LogRow {
    seq: u64,
    at: i64,
    actor: String,
    action: String,
    subject: Option<String>,
    request: String,
    detail: String,
}

impl LogRow {
    fn entry(self) -> GovernanceEntry {
        let detail = serde_json::from_str(&self.detail).unwrap_or(Value::Null);
        GovernanceEntry { seq: self.seq, at: self.at, actor: self.actor, action: self.action, subject: self.subject, request: self.request, detail }
    }
}

fn refused(r: Refusal) -> CellError {
    CellError::new(r.code(), r.message())
}

/// A client's id, kept prefixed by its kind.
fn client_request(kind: &str, id: &str) -> CellResult<String> {
    match governance::valid_request(id) {
        true => Ok(format!("{kind}:{id}")),
        false => Err(CellError::invalid(format!("an id is 1-{} printable ASCII characters, no spaces", governance::REQUEST_MAX_BYTES))),
    }
}

impl RegistryCell {
    /// Runs one change's statements in one `transactionSync`.
    fn atomically<T: 'static>(&self, f: impl FnOnce(&Tx) -> CellResult<T> + 'static) -> CellResult<T> {
        let sql = self.state.storage().sql();
        js::transaction_sync(&self.raw, move || f(&Tx(sql)))
    }

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

    /// The latest email `person` signed in with.
    fn email_now(&self, person: &str) -> CellResult<Option<String>> {
        #[derive(Deserialize)]
        struct Row {
            email: String,
        }
        Ok(self.row::<Row>("SELECT email FROM subjects WHERE identity = ? ORDER BY signed_in_at DESC LIMIT 1", vec![person.into()])?.map(|r| r.email))
    }

    fn offer_view(&self, org: &OrgRow, r: OfferRow) -> CellResult<ContentOffer> {
        let from = if npub::is_identity(&r.offered_by) { self.email_now(&r.offered_by)? } else { None };
        let answer = match r.standing(js::now_ms())? {
            Standing::Expired => Some("expired".to_string()),
            _ => r.answer,
        };
        Ok(ContentOffer { id: r.id, org: org.reference(), email: r.email, grounds: r.grounds, from, at: r.offered_at, expires_at: r.expires_at, answer })
    }

    fn offer_admin(&self, b: OfferAdmin) -> CellResult<ContentOffer> {
        let request = client_request("offer", &b.request)?;
        let org = self.content_org(&b.org)?;
        let email = super::signin::email_of(&b.email).ok_or_else(|| CellError::invalid(format!("{:?} is not an email", b.email)))?;
        // who offers and who pays, read in this turn: the change below runs in it too
        let payer = self.payer_of(&org.id)?;
        let (actor, offerer) = match (&b.by, &b.operator) {
            (None, Some(op)) => (op.clone(), Some(Grounds::Operator)),
            (Some(by), None) => {
                let who = self.governor(by)?;
                let admin = self.row::<Value>("SELECT 1 AS one FROM org_content WHERE org = ? AND person = ? AND admin = 1", vec![org.id.as_str().into(), who.id.as_str().into()])?.is_some();
                let pays = payer.as_deref() == Some(who.id.as_str());
                let offerer = match (admin, pays) {
                    (true, _) => Some(Grounds::Admin),
                    (false, true) => Some(Grounds::Payer),
                    (false, false) => None,
                };
                (who.id, offerer)
            }
            _ => return Err(CellError::host("an offer is a person's or an operator's")),
        };
        let id = self.atomically({
            let (org, actor) = (org.id.clone(), actor.clone());
            move |tx| {
                let now = js::now_ms();
                // a replay: the offer its entry names, made by the same for the same
                if let Some(e) = tx.governed(&org, &request)? {
                    let detail: Value = serde_json::from_str(&e.detail).unwrap_or(Value::Null);
                    let offer = detail["offer"].as_str().map(str::to_string);
                    return match (offer, e.actor == actor) {
                        (Some(offer), true) if tx.offer(&org, &offer)?.email == email => Ok(offer),
                        _ => Err(CellError::new(ErrorCode::ConflictingBody, "this id was used for another change")),
                    };
                }
                tx.expire(&org, now)?;
                let lapsed = tx.settle(&org, payer.as_deref(), now)?;
                let waiting = tx.count("SELECT COUNT(*) AS n FROM org_content_offers WHERE org = ? AND answer IS NULL AND grounds != 'operator'", vec![org.as_str().into()])?;
                let grounds = governance::may_offer(offerer, tx.people(&org)?, tx.admins(&org)?, waiting).map_err(refused)?;
                // offered and waiting already: this request names that one
                let existing: Option<OfferRow> =
                    tx.row(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE org = ? AND email = ? AND answer IS NULL"), vec![org.as_str().into(), email.as_str().into()])?;
                let (id, kept) = match existing {
                    Some(o) => (o.id, o.grounds),
                    None => {
                        let id = fresh_id("cof-");
                        tx.exec(
                            "INSERT INTO org_content_offers (id, org, email, grounds, offered_by, offered_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                            vec![
                                id.as_str().into(),
                                org.as_str().into(),
                                email.as_str().into(),
                                grounds.as_str().into(),
                                actor.as_str().into(),
                                SqlStorageValue::Integer(now),
                                SqlStorageValue::Integer(now + OFFER_TTL_MS),
                            ],
                        )?;
                        (id, grounds.as_str().to_string())
                    }
                };
                // the offer's own grounds (one waiting already keeps its own)
                let detail = json!({ "offer": id, "grounds": kept, "lapsed": lapsed });
                tx.govern(&org, Logged { request: &request, actor: &actor, action: "admin-offered", subject: Some(&id), detail }, now)?;
                if grounds == Grounds::Operator {
                    // the operators' log too, in the same change
                    tx.exec(
                        "INSERT INTO admin_log (at, operator, action, target, detail) VALUES (?, ?, 'content-admin-offer', ?, ?)",
                        vec![SqlStorageValue::Integer(now), actor.as_str().into(), org.as_str().into(), format!("offer {id}").into()],
                    )?;
                }
                Ok(id)
            }
        })?;
        let row = self.row::<OfferRow>(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE id = ?"), vec![id.as_str().into()])?.ok_or_else(|| CellError::host("an offer made is not there"))?;
        self.offer_view(&org, row)
    }

    fn answer_offer(&self, b: AnswerOffer) -> CellResult<ContentOffer> {
        let who = self.governor(&b.by)?;
        let org = self.content_org(&b.org)?;
        let payer = self.payer_of(&org.id)?;
        let id = self.atomically({
            let (org, person, offer) = (org.id.clone(), who.id.clone(), b.offer.clone());
            move |tx| {
                let now = js::now_ms();
                let o = tx.offer(&org, &offer)?;
                // only whoever signs in with its email sees it
                if tx.count("SELECT COUNT(*) AS n FROM subjects WHERE identity = ? AND email = ?", vec![person.as_str().into(), o.email.as_str().into()])? == 0 {
                    return Err(not_found(format!("no offer {offer}")));
                }
                if !governance::answer(o.standing(now)?, b.answer).map_err(refused)? {
                    return Ok(o.id);
                }
                if b.answer == Answer::Accepted {
                    let grounds = o.grounds()?;
                    let offerer_stands = match grounds {
                        Grounds::Admin => tx.place(&org, &o.offered_by)? == Some(true),
                        Grounds::Payer => payer.as_deref() == Some(o.offered_by.as_str()),
                        Grounds::Operator => true,
                    };
                    let now_is = Now { people: tx.people(&org)?, admins: tx.admins(&org)?, offerer_stands };
                    governance::grounds_hold(grounds, now_is).map_err(refused)?;
                    // made before they last left: it admits them no more
                    let was = tx.place_of(&org, &person)?;
                    let place = governance::accepted(was.map(|w| w.0), was.and_then(|w| w.1), o.offered_at).map_err(refused)?;
                    tx.write_place(&org, &person, was.map(|w| w.0), place, now)?;
                }
                tx.exec(
                    "UPDATE org_content_offers SET answer = ?, answered_by = ?, answered_at = ? WHERE id = ? AND answer IS NULL",
                    vec![b.answer.as_str().into(), person.as_str().into(), SqlStorageValue::Integer(now), o.id.as_str().into()],
                )?;
                // an accept can make others lapse (who pays's other "firsts",
                // an operator's recovery): settled with it, named in its entry
                let lapsed = tx.settle(&org, payer.as_deref(), now)?;
                let action = format!("admin-{}", b.answer.as_str());
                let detail = json!({ "offer": o.id, "lapsed": lapsed });
                tx.govern(&org, Logged { request: &format!("answer:{}", o.id), actor: &person, action: &action, subject: Some(&person), detail }, now)?;
                Ok(o.id)
            }
        })?;
        let row = self.row::<OfferRow>(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE id = ?"), vec![id.as_str().into()])?.ok_or_else(|| CellError::host("an offer answered is not there"))?;
        self.offer_view(&org, row)
    }

    fn withdraw_offer(&self, b: WithdrawOffer) -> CellResult<ContentOffer> {
        let who = self.governor(&b.by)?;
        let org = self.content_org(&b.org)?;
        self.atomically({
            let (org, person, offer) = (org.id.clone(), who.id.clone(), b.offer.clone());
            move |tx| {
                let now = js::now_ms();
                if tx.place(&org, &person)? != Some(true) {
                    return Err(refused(Refusal::NotAdmin));
                }
                let o = tx.offer(&org, &offer)?;
                match o.standing(now)? {
                    Standing::Waiting => {}
                    Standing::Withdrawn => return Ok(()),
                    Standing::Answered(a) => return Err(refused(Refusal::Answered(a))),
                    Standing::Expired | Standing::Lapsed => return Err(refused(Refusal::Ended)),
                }
                tx.exec(
                    "UPDATE org_content_offers SET answer = 'withdrawn', answered_by = ?, answered_at = ? WHERE id = ? AND answer IS NULL",
                    vec![person.as_str().into(), SqlStorageValue::Integer(now), o.id.as_str().into()],
                )?;
                tx.govern(&org, Logged { request: &format!("withdraw:{}", o.id), actor: &person, action: "admin-offer-withdrawn", subject: Some(&o.id), detail: json!({ "offer": o.id }) }, now)
            }
        })?;
        let row = self.row::<OfferRow>(&format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE id = ?"), vec![b.offer.as_str().into()])?.ok_or_else(|| CellError::host("an offer withdrawn is not there"))?;
        self.offer_view(&org, row)
    }

    fn offers_to_me(&self, b: OffersToMe) -> CellResult<ContentOffers> {
        let who = self.governor(&b.by)?;
        // bounded: at most 8 emails a person, each offered once an org; an
        // offer of an org that is gone is no one's to answer, and none shows
        let rows = self.rows::<OfferRow>(
            &format!(
                "SELECT {OFFER_COLUMNS} FROM org_content_offers o WHERE answer IS NULL AND expires_at > ? AND EXISTS (SELECT 1 FROM orgs WHERE id = o.org)
                   AND email IN (SELECT email FROM subjects WHERE identity = ?) ORDER BY offered_at, id LIMIT 100"
            ),
            vec![SqlStorageValue::Integer(js::now_ms()), who.id.as_str().into()],
        )?;
        let mut offers = Vec::with_capacity(rows.len());
        for r in rows {
            let org = self.content_org(&r.org)?;
            offers.push(self.offer_view(&org, r)?);
        }
        Ok(ContentOffers { offers })
    }

    fn step_down(&self, b: StepDown) -> CellResult<ContentPerson> {
        let who = self.governor(&b.by)?;
        let org = self.content_org(&b.org)?;
        self.atomically({
            let (org, actor, person) = (org.id.clone(), who.id.clone(), b.person.clone());
            move |tx| {
                let now = js::now_ms();
                if tx.place(&org, &actor)? != Some(true) {
                    return Err(refused(Refusal::NotAdmin));
                }
                let admin = tx.place(&org, &person)?.ok_or_else(|| refused(Refusal::NotIn))?;
                if !governance::steps_down(admin, tx.admins(&org)?).map_err(refused)? {
                    return Ok(());
                }
                tx.exec("UPDATE org_content SET admin = 0 WHERE org = ? AND person = ?", vec![org.as_str().into(), person.as_str().into()])?;
                let withdrawn = tx.retire_offers(&org, &person, &actor, now)?;
                let request = format!("step-down:{}", hex::encode(js::random_bytes::<8>()));
                tx.govern(&org, Logged { request: &request, actor: &actor, action: "admin-stepped-down", subject: Some(&person), detail: json!({ "withdrawn": withdrawn }) }, now)
            }
        })?;
        self.person_view(&org.id, &b.person)
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
            .ok_or_else(|| refused(Refusal::NotIn))?;
        Ok(ContentPerson { person: person.to_string(), email: self.email_now(person)?, epoch: r.epoch, active: r.active != 0, admin: r.admin != 0, since: r.since })
    }

    fn offboard(&self, b: OffboardCall) -> CellResult<ContentPerson> {
        let request = client_request("offboard", &b.request)?;
        let who = self.governor(&b.by)?;
        let org = self.content_org(&b.org)?;
        self.atomically({
            let (org, actor, person) = (org.id.clone(), who.id.clone(), b.person.clone());
            move |tx| {
                let now = js::now_ms();
                // a replay first: it answers as it did, whatever came since
                if let Some(e) = tx.governed(&org, &request)? {
                    return match e.action == "offboarded" && e.subject.as_deref() == Some(person.as_str()) && e.actor == actor {
                        true => Ok(()),
                        false => Err(CellError::new(ErrorCode::ConflictingBody, "this id was used for another change")),
                    };
                }
                if tx.place(&org, &actor)? != Some(true) {
                    return Err(refused(Refusal::NotAdmin));
                }
                let (was, _) = tx.place_of(&org, &person)?.ok_or_else(|| refused(Refusal::NotIn))?;
                let next = governance::offboarded(was, b.expected_epoch, tx.admins(&org)?).map_err(refused)?;
                tx.write_place(&org, &person, Some(was), next, now)?;
                let withdrawn = tx.retire_offers(&org, &person, &actor, now)?;
                tx.govern(&org, Logged { request: &request, actor: &actor, action: "offboarded", subject: Some(&person), detail: json!({ "from": was.epoch, "to": next.epoch, "withdrawn": withdrawn }) }, now)
            }
        })?;
        console_log!("{}", json!({ "event": "org.offboarded", "org": org.id, "person": b.person }));
        self.person_view(&org.id, &b.person)
    }

    fn enroll_content(&self, b: Enroll) -> CellResult<Enrolled> {
        let org = self.content_org(&b.org)?;
        if !npub::is_identity(&b.person) {
            return Err(CellError::invalid("enroll names a person by identity"));
        }
        self.atomically({
            let org = org.id.clone();
            move |tx| {
                let now = js::now_ms();
                let was = tx.place_of(&org, &b.person)?;
                let (place, joins) = governance::enrolled(was.map(|w| w.0), was.and_then(|w| w.1), b.invited_at).map_err(refused)?;
                if joins {
                    tx.write_place(&org, &b.person, was.map(|w| w.0), place, now)?;
                    let request = format!("join:{}:{}", b.person, place.epoch);
                    tx.govern(&org, Logged { request: &request, actor: &b.person, action: "joined", subject: Some(&b.person), detail: json!({ "epoch": place.epoch }) }, now)?;
                }
                Ok(Enrolled { epoch: place.epoch })
            }
        })
    }

    fn content_of(&self, b: ContentOf) -> CellResult<OrgContent> {
        let org = self.content_org(&b.org)?;
        if let Some(by) = &b.by {
            let who = self.governor(by)?;
            if self.row::<Value>("SELECT 1 AS one FROM org_content WHERE org = ? AND person = ? AND admin = 1", vec![org.id.as_str().into(), who.id.as_str().into()])?.is_none() {
                return Err(refused(Refusal::NotAdmin));
            }
        }
        #[derive(Deserialize)]
        struct Person {
            person: String,
            epoch: u64,
            active: i64,
            admin: i64,
            since: i64,
            email: Option<String>,
        }
        // bounded: a page of the people who said yes to it
        let people = self.rows::<Person>(
            "SELECT c.person, c.epoch, c.active, c.admin, c.since, (SELECT email FROM subjects s WHERE s.identity = c.person ORDER BY signed_in_at DESC LIMIT 1) AS email
             FROM org_content c WHERE c.org = ? ORDER BY c.since, c.person LIMIT 1000",
            vec![org.id.as_str().into()],
        )?;
        let people = people.into_iter().map(|p| ContentPerson { person: p.person, email: p.email, epoch: p.epoch, active: p.active != 0, admin: p.admin != 0, since: p.since }).collect();
        let offers = self.rows::<OfferRow>(
            &format!("SELECT {OFFER_COLUMNS} FROM org_content_offers WHERE org = ? AND answer IS NULL AND expires_at > ? ORDER BY offered_at, id"),
            vec![org.id.as_str().into(), SqlStorageValue::Integer(js::now_ms())],
        )?;
        let offers = offers.into_iter().map(|r| self.offer_view(&org, r)).collect::<CellResult<Vec<_>>>()?;
        let before = i64::try_from(b.before.unwrap_or(i64::MAX as u64)).map_err(|_| CellError::invalid("before is a log entry's number"))?;
        let log = self.rows::<LogRow>(
            "SELECT seq, at, actor, action, subject, request, detail FROM org_governance WHERE org = ? AND seq < ? ORDER BY seq DESC LIMIT ?",
            vec![org.id.as_str().into(), SqlStorageValue::Integer(before), SqlStorageValue::Integer(LOG_PAGE as i64)],
        )?;
        let next = (log.len() as u64 == LOG_PAGE).then(|| log.last().map(|e| e.seq)).flatten();
        Ok(OrgContent { org: org.reference(), people, offers, log: log.into_iter().map(LogRow::entry).collect(), next })
    }

    fn nominate_payer(&self, b: NominatePayer) -> CellResult<PayerNamed> {
        let request = client_request("payer", &b.request)?;
        let org = self.content_org(&b.org)?;
        // a person, by an email they sign in with or their npub
        let person = match fragment_core::mail::valid_address(&b.person) {
            true => self.person_by_email(&b.person)?,
            false => npub::parse(&b.person).map(|k| npub::identity_of(&k)).filter(|id| self.identity(id).ok().flatten().is_some_and(|i| i.kind == IdentityKind::Person)),
        }
        .ok_or_else(|| not_found(format!("{} names no person on this fleet", b.person)))?;
        let seq = self.atomically({
            let (org, operator, person) = (org.id.clone(), b.operator.clone(), person.clone());
            move |tx| {
                let now = js::now_ms();
                if let Some(e) = tx.governed(&org, &request)? {
                    return match e.action == "payer-named" && e.subject.as_deref() == Some(person.as_str()) && e.actor == operator {
                        true => Ok(e.seq),
                        false => Err(CellError::new(ErrorCode::ConflictingBody, "this id was used for another change")),
                    };
                }
                tx.govern(&org, Logged { request: &request, actor: &operator, action: "payer-named", subject: Some(&person), detail: json!({}) }, now)?;
                // the operators' log too, in the same change
                tx.exec(
                    "INSERT INTO admin_log (at, operator, action, target, detail) VALUES (?, ?, 'org-payer', ?, ?)",
                    vec![SqlStorageValue::Integer(now), operator.as_str().into(), org.as_str().into(), person.as_str().into()],
                )?;
                Ok(tx.governed(&org, &request)?.ok_or_else(|| CellError::host("an entry just written is not there"))?.seq)
            }
        })?;
        Ok(PayerNamed { org: org.id, email: self.email_now(&person)?, person, seq })
    }

    /// Whether `org` has anyone in its content: its ledger and estate
    /// outlive its billing (decision 69), so it is never dropped with it.
    pub(super) fn has_content(&self, org: &str) -> CellResult<bool> {
        Ok(self.count("SELECT COUNT(*) AS n FROM org_content WHERE org = ?", vec![org.into()])? > 0)
    }

    /// An org dropped with no one in its content: its offers go with it,
    /// in the drop's turn, so none names an org that is not there.
    pub(super) fn content_dropped(&self, org: &str) -> CellResult<()> {
        self.exec("DELETE FROM org_content_offers WHERE org = ?", vec![org.into()])
    }

    /// A wiped person, in every org's content: an admin no more, and their
    /// waiting offers, made or received, withdrawn, each logged as the
    /// operator who wiped them, once an org (run again, it changes
    /// nothing). Their rows stay, the audit trail (Paul, 2026-10-30). An org
    /// left with no admin waits on an operator's recovery.
    pub(super) fn content_wiped(&self, person: &str, operator: &str) -> CellResult<()> {
        #[derive(Deserialize)]
        struct Row {
            org: String,
        }
        // bounded: the orgs whose content they were in. Each is its own
        // change, done once: a wipe whose step failed after some (Alex's
        // review) is run again, and finds those done by their entries. Out
        // at a raised epoch, as an offboarding is, if they were still in
        for r in self.rows::<Row>("SELECT org FROM org_content WHERE person = ?", vec![person.into()])? {
            let (person, operator) = (person.to_string(), operator.to_string());
            self.atomically(move |tx| {
                let request = format!("wipe:{person}");
                if tx.governed(&r.org, &request)?.is_some() {
                    return Ok(());
                }
                let now = js::now_ms();
                let (was, _) = tx.place_of(&r.org, &person)?.ok_or_else(|| CellError::host("a place just read is not there"))?;
                if was.active {
                    tx.write_place(&r.org, &person, Some(was), Place { epoch: was.epoch + 1, active: false, admin: false }, now)?;
                }
                let withdrawn = tx.retire_offers(&r.org, &person, &operator, now)?;
                tx.govern(&r.org, Logged { request: &request, actor: &operator, action: "wiped", subject: Some(&person), detail: json!({ "withdrawn": withdrawn }) }, now)
            })?;
        }
        Ok(())
    }

    pub(super) fn content_route(&self, path: &str, bytes: &[u8]) -> Option<CellResult<Response>> {
        Some(match path {
            OfferAdmin::PATH => reply::<OfferAdmin>(body(bytes).and_then(|b| self.offer_admin(b))),
            AnswerOffer::PATH => reply::<AnswerOffer>(body(bytes).and_then(|b| self.answer_offer(b))),
            WithdrawOffer::PATH => reply::<WithdrawOffer>(body(bytes).and_then(|b| self.withdraw_offer(b))),
            OffersToMe::PATH => reply::<OffersToMe>(body(bytes).and_then(|b| self.offers_to_me(b))),
            StepDown::PATH => reply::<StepDown>(body(bytes).and_then(|b| self.step_down(b))),
            OffboardCall::PATH => reply::<OffboardCall>(body(bytes).and_then(|b| self.offboard(b))),
            ContentOf::PATH => reply::<ContentOf>(body(bytes).and_then(|b| self.content_of(b))),
            Enroll::PATH => reply::<Enroll>(body(bytes).and_then(|b| self.enroll_content(b))),
            NominatePayer::PATH => reply::<NominatePayer>(body(bytes).and_then(|b| self.nominate_payer(b))),
            _ => return None,
        })
    }
}
