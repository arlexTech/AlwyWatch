// SPDX-License-Identifier: AGPL-3.0-or-later
//! Jellyfin users made from invitations, with the server's API key (`JELLYFIN_API_KEY`).
//! Endpoints checked against Jellyfin's OpenAPI: `GET /Users`, `POST /Users/New`
//! (CreateUserByName: Name, Password), `GET /Users/{id}`, `POST /Users/{id}/Policy`,
//! `DELETE /Users/{id}`, `POST /Users/AuthenticateByName`, `POST /Sessions/Logout`.

use net::reqwest::{self, Client, Method};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::ServerConf;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct JfUser {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub policy: Option<JfPolicy>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct JfPolicy {
    #[serde(default)]
    pub is_administrator: bool,
    #[serde(default)]
    pub is_disabled: bool,
}

impl JfUser {
    /// A person to copy library access from: not an admin, not disabled.
    pub fn is_regular(&self) -> bool {
        self.policy
            .as_ref()
            .is_some_and(|p| !p.is_administrator && !p.is_disabled)
    }
}

pub struct Jellyfin<'a> {
    http: &'a Client,
    server: &'a ServerConf,
    key: &'a str,
}

const CLIENT: &str = "MediaBrowser Client=\"AlwyWatch\", Device=\"AlwyWatch server\", \
                      DeviceId=\"alwywatch-server-invitations\", Version=\"1\"";

impl<'a> Jellyfin<'a> {
    pub fn new(http: &'a Client, server: &'a ServerConf, key: &'a str) -> Self {
        Self { http, server, key }
    }

    fn req(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.server.internal()))
            .header(
                "Authorization",
                format!("MediaBrowser Token=\"{}\"", self.key),
            )
            .header("Accept", "application/json")
    }

    pub async fn users(&self) -> Result<Vec<JfUser>, net::Error> {
        net::send_json(self.req(Method::GET, "/Users")).await
    }

    /// A new user with this password: its id.
    pub async fn create(&self, name: &str, password: &str) -> Result<String, net::Error> {
        let u: JfUser = net::send_json(
            self.req(Method::POST, "/Users/New")
                .json(&json!({ "Name": name, "Password": password })),
        )
        .await?;
        Ok(u.id)
    }

    /// Gives `to` the same access as `from` (libraries, ratings, downloads…), never admin.
    pub async fn copy_policy(&self, from: &str, to: &str) -> Result<(), net::Error> {
        let u: Value = net::send_json(self.req(
            Method::GET,
            &format!("/Users/{}", net::percent_encode(from)),
        ))
        .await?;
        let Some(mut p) = u.get("Policy").cloned() else {
            return Ok(());
        };
        if let Some(o) = p.as_object_mut() {
            o.insert("IsAdministrator".into(), false.into());
            o.insert("IsDisabled".into(), false.into());
            o.insert("IsHidden".into(), true.into());
            o.insert("InvalidLoginAttemptCount".into(), 0.into());
            // Their own provider ids and devices, not the template's.
            o.remove("AuthenticationProviderId");
            o.remove("PasswordResetProviderId");
            o.insert("EnableAllDevices".into(), true.into());
            o.insert("EnabledDevices".into(), Value::Array(vec![]));
        }
        send_empty(
            self.req(
                Method::POST,
                &format!("/Users/{}/Policy", net::percent_encode(to)),
            )
            .json(&p),
        )
        .await
    }

    pub async fn delete(&self, id: &str) -> Result<(), net::Error> {
        send_empty(self.req(
            Method::DELETE,
            &format!("/Users/{}", net::percent_encode(id)),
        ))
        .await
    }

    /// Signs in as the new user once (to learn who they are the way every sign-in does), then
    /// ends that session: the server keeps no token.
    pub async fn sign_in_once(&self, name: &str, password: &str) -> Result<String, net::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Auth {
            access_token: String,
        }
        let a: Auth = net::send_json(
            self.http
                .post(format!(
                    "{}/Users/AuthenticateByName",
                    self.server.internal()
                ))
                .header("Authorization", CLIENT)
                .header("Accept", "application/json")
                .json(&json!({ "Username": name, "Pw": password })),
        )
        .await?;
        Ok(a.access_token)
    }

    pub async fn sign_out(&self, token: &str) {
        let _ = send_empty(
            self.http
                .post(format!("{}/Sessions/Logout", self.server.internal()))
                .header("Authorization", format!("{CLIENT}, Token=\"{token}\"")),
        )
        .await;
    }
}

async fn send_empty(req: reqwest::RequestBuilder) -> Result<(), net::Error> {
    net::send(req).await.map(|_| ())
}
