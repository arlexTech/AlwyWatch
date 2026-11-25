// SPDX-License-Identifier: AGPL-3.0-or-later
//! Play on the TV (`keep_service::remote`): the TV side listens and reports, the phone side
//! lists the person's TVs, starts a title on one and controls it. Everything goes through the
//! AlwyWatch server this profile is connected to.

use keep_service::remote::{
    RemoteAction, RemoteCommand, RemoteCommands, RemoteDevice, RemoteReport, RemoteState,
};

use crate::{AlwyWatch, Result};

/// Where a TV plays a Play command: one of its accounts and the item there.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RemotePlay {
    pub account_id: String,
    pub item_id: String,
    /// Where to start (ms); None: where this account's server says (resume or start).
    pub position_ms: Option<i64>,
}

#[uniffi::export]
impl AlwyWatch {
    /// This TV's id for Play on the TV: stable for the install and the profile.
    pub fn remote_device_id(&self) -> String {
        format!("{}-{}", self.core.client.device_id, self.core.profile)
    }

    /// TV: commands past `after` (0 the first time: the answer then only sets `last_seq`), as
    /// soon as there are any or after about 25 s. Call it in a loop while AlwyWatch is open on
    /// the TV with this profile: it is also how the person's phones see the TV. `name` is what
    /// the phones show ("Living room TV").
    pub async fn remote_listen(&self, name: String, after: u64) -> Result<RemoteCommands> {
        let device = self.remote_device_id();
        self.run(|c| async move { Ok(c.keep()?.remote_listen(&device, &name, after).await?) })
            .await
    }

    /// TV: what it plays now (an empty title: nothing). Send it on every change, and every
    /// few seconds while playing.
    pub async fn remote_report(&self, report: RemoteReport) -> Result<()> {
        let device = self.remote_device_id();
        self.run(|c| async move { Ok(c.keep()?.remote_report(&device, &report).await?) })
            .await
    }

    /// TV: no longer there (the app goes to the background, or another profile opens).
    pub async fn remote_leave(&self) -> Result<()> {
        let device = self.remote_device_id();
        self.run(|c| async move { Ok(c.keep()?.remote_leave(&device).await?) })
            .await
    }

    /// TV: where to play a Play command: the same item when this profile is signed in to the
    /// phone's server, else the same title found on another of its servers (by TMDB/IMDB/TVDB
    /// ids, then by name). None: on none of them.
    pub async fn remote_resolve(&self, command: RemoteCommand) -> Result<Option<RemotePlay>> {
        let accounts = self.accounts()?;
        let position_ms = command.position_ms;
        if let (Some(server), Some(item)) = (&command.server_id, &command.item_id)
            && let Some(a) = accounts.iter().find(|a| &a.server_id == server)
        {
            return Ok(Some(RemotePlay {
                account_id: a.id.clone(),
                item_id: item.clone(),
                position_ms,
            }));
        }
        let Some(media) = command.media else {
            return Ok(None);
        };
        for a in accounts {
            let account_id = a.id.clone();
            let m = media.clone();
            let found = self
                .run(|c| async move { c.together_find_on(&account_id, &m).await })
                .await;
            if let Ok(Some(item_id)) = found {
                return Ok(Some(RemotePlay {
                    account_id: a.id,
                    item_id,
                    // Another server: its own resume point, not the phone's.
                    position_ms: None,
                }));
            }
        }
        Ok(None)
    }

    /// Phone: my TVs there now (AlwyWatch open on them with my profile).
    pub async fn remote_devices(&self) -> Result<Vec<RemoteDevice>> {
        self.run(|c| async move { Ok(c.keep()?.remote_devices().await?) })
            .await
    }

    /// Phone: plays `item_id` of `account_id` on the TV `device_id`, from `position_ms` (None:
    /// where the TV's server says).
    pub async fn remote_play(
        &self,
        device_id: String,
        account_id: String,
        item_id: String,
        position_ms: Option<i64>,
    ) -> Result<()> {
        let server_id = self
            .accounts()?
            .into_iter()
            .find(|a| a.id == account_id)
            .map(|a| a.server_id);
        self.run(|c| async move {
            let media = c.together_media(&account_id, &item_id).await?;
            let command = RemoteCommand {
                seq: 0,
                action: RemoteAction::Play,
                media: Some(media),
                server_id,
                item_id: Some(item_id),
                position_ms,
                index: None,
            };
            Ok(c.keep()?.remote_command(&device_id, &command).await?)
        })
        .await
    }

    /// Phone: pause, resume, seek (`position_ms`), stop, audio or subtitles (`index`; None
    /// with Subtitles: off) on the TV `device_id`.
    pub async fn remote_control(
        &self,
        device_id: String,
        action: RemoteAction,
        position_ms: Option<i64>,
        index: Option<i32>,
    ) -> Result<()> {
        let command = RemoteCommand {
            seq: 0,
            action,
            media: None,
            server_id: None,
            item_id: None,
            position_ms,
            index,
        };
        self.run(|c| async move { Ok(c.keep()?.remote_command(&device_id, &command).await?) })
            .await
    }

    /// Phone: the TV's state once its `version` differs from `after` (0 the first time), or
    /// after about 25 s. Loop on it while the remote is open; `NotFound`: the TV is gone.
    pub async fn remote_state(&self, device_id: String, after: u64) -> Result<RemoteState> {
        self.run(|c| async move { Ok(c.keep()?.remote_state(&device_id, after).await?) })
            .await
    }
}

/// Where a TV's title is now (ms): its report moved on while it plays.
#[uniffi::export]
pub fn remote_position_now(state: &RemoteState) -> i64 {
    state.position_now()
}
