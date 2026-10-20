//! The shell's support launcher and directory. Ordinary private chat
//! fragments own the transcript, live delivery, unread counts and access.
use fragment_proto::{CreateFragment, ErrorCode, FragmentStatus, Visibility};
use serde_json::json;
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::{
    calls::{By, Lookup, Profiles},
    support::{Directory, Thread},
};
use crate::routed::{Routed, Signed};

async fn call(
    env: &Env,
    url: &Url,
    t: &Thread,
    owner: &Signed,
    method: Method,
    inner: &str,
    body: Option<serde_json::Value>,
) -> CellResult<Response> {
    let req = Request::new(url.as_str(), method)?;
    crate::forward(
        env,
        &req,
        body.map(|v| v.to_string().into()),
        crate::Forward {
            routed: Routed {
                name: t.name.clone(),
                url: url.clone(),
                signed: Some(owner.clone()),
                credential: None,
            },
            inner: inner.into(),
            extra: vec![],
        },
    )
    .await
}

async fn checked(mut response: Response) -> CellResult<Response> {
    if response.status_code() >= 400 {
        let status = response.status_code();
        let body = response.json::<fragment_proto::ErrorBody>().await?;
        return Err(CellError::new(
            body.error,
            format!("support ({status}): {}", body.message),
        ));
    }
    Ok(response)
}

pub(crate) async fn route(
    mut req: Request,
    env: &Env,
    cfg: &Config,
    url: &Url,
    rest: &[&str],
) -> CellResult<Response> {
    let bytes = crate::read_body(&mut req, 1024).await?;
    if !bytes.is_empty() {
        return Err(CellError::invalid("support requests take no body"));
    }
    let who = crate::signer(env, &req, url, &bytes).await?;
    let (person, open, info) = match (req.method(), rest) {
        (Method::Get, []) => (None, false, false),
        (Method::Post, []) => (Some(who.id.clone()), true, false),
        (Method::Post, [person]) => (Some(person.to_string()), true, false),
        (Method::Get, [person, "info"]) => (Some(person.to_string()), false, true),
        _ => return Err(crate::no_route(url.path())),
    };
    let d = crate::ask_registry(
        env,
        &Directory {
            by: By::Identity(who.id.clone()),
            person,
            create: open && rest.is_empty(),
            after: url
                .query_pairs()
                .find(|(k, _)| k == "after")
                .map(|(_, v)| v.into_owned()),
        },
    )
    .await?;
    if info {
        if !d.staff {
            return Err(CellError::new(
                ErrorCode::Forbidden,
                "only the support team can read customer details",
            ));
        }
        let t = d
            .threads
            .first()
            .ok_or_else(|| CellError::new(ErrorCode::NotFound, "no support thread"))?;
        let person = crate::ask_registry(
            env,
            &crate::registry::admin::Person {
                person: t.person.clone(),
            },
        )
        .await?;
        let mut profiles = crate::ask_registry(
            env,
            &Profiles {
                ids: vec![t.person.clone()],
                emails_of: vec![],
                agents_of: vec![t.person.clone()],
            },
        )
        .await?;
        crate::share::name_agents(env, &mut profiles.profiles).await;
        return crate::json_answer(
            &json!({ "email": person.email, "org": person.org, "agents": profiles.profiles.get(&t.person).map(|p| &p.agents) }),
        );
    }
    if open {
        let t = d
            .threads
            .first()
            .ok_or_else(|| CellError::new(ErrorCode::NotFound, "no support thread"))?;
        let identity = crate::ask_registry(
            env,
            &Lookup {
                who: t.owner.clone(),
            },
        )
        .await?;
        let owner = Signed::new(identity, None);
        let mut status = call(env, url, t, &owner, Method::Get, "/api/status", None).await?;
        if status.status_code() == 404 {
            let created = crate::create_fragment(
                env,
                cfg,
                url,
                CreateFragment {
                    name: t.name.clone(),
                    label: None,
                    visibility: Some(Visibility::Members),
                    template: Some("chat".into()),
                    title: Some("Finite Support".into()),
                },
                owner.clone(),
            )
            .await?;
            // A concurrent opener may have created the same reservation.
            if created.status_code() != ErrorCode::AlreadyExists.status() {
                checked(created).await?;
            }
            status = call(env, url, t, &owner, Method::Get, "/api/status", None).await?;
        }
        let s: FragmentStatus = checked(status).await?.json().await?;
        if s.owner != t.owner || s.visibility != Visibility::Members {
            return Err(CellError::new(
                ErrorCode::Forbidden,
                "support conversation ownership or privacy changed; email support@finite.computer",
            ));
        }
        // Only these server-selected people are granted access, never ids supplied by a client.
        let mut members: Vec<String> = d.team.iter().map(|p| p.id.clone()).collect();
        members.push(t.person.clone());
        members.sort();
        members.dedup();
        for id in members.into_iter().filter(|id| id != &t.owner) {
            checked(
                call(
                    env,
                    url,
                    t,
                    &owner,
                    Method::Put,
                    &format!("/api/members/{id}"),
                    Some(json!({ "role": "viewer" })),
                )
                .await?,
            )
            .await?;
        }
        return crate::json_answer(
            &json!({ "name": t.name, "person": t.person, "staff": d.staff }),
        );
    }
    let list = crate::listed(env, &who.id).await?;
    let ids: Vec<String> = d.threads.iter().map(|t| t.person.clone()).collect();
    let profiles = crate::ask_registry(
        env,
        &Profiles {
            ids: ids.clone(),
            emails_of: ids,
            agents_of: vec![],
        },
    )
    .await?;
    let threads: Vec<_> = d.threads.iter().map(|t| {
        let f = list.fragments.iter().find(|f| f.name == t.name);
        json!({ "person": t.person, "name": t.name,
            "email": profiles.profiles.get(&t.person).and_then(|p| p.email.as_deref()),
            "preview": f.and_then(|f| f.preview.as_deref()), "at": f.and_then(|f| f.preview_at).unwrap_or(t.created_at), "unread": f.map_or(0, |f| f.unread) })
    }).collect();
    crate::json_answer(&json!({ "staff": d.staff, "threads": threads, "next": d.next }))
}
