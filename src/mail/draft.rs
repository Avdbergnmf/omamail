//! Save a composed message as a provider draft, or remove one. Nothing here can
//! send: a draft is the same validated message `send` would build, stored in the
//! account's Drafts instead of the outbox. Previews by default, like every
//! mutation.
use super::{Account, Provider, SendRequest};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin};

type Result<T> = std::result::Result<T, &'static str>;
type Pending<'a> = Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>>;

pub struct DraftRequest {
    pub message: SendRequest,
    /// An existing draft this one replaces (the provider removes the old copy).
    pub replace: Option<String>,
}

pub struct DeleteDraftRequest {
    pub account: Account,
    pub id: String,
    pub execute: bool,
}

pub(crate) trait DraftStore: Send + Sync {
    fn save<'a>(
        &'a self,
        account: &'a Account,
        raw: &'a str,
        replace: Option<&'a str>,
    ) -> Pending<'a>;
    fn delete<'a>(&'a self, account: &'a Account, id: &'a str) -> Pending<'a>;
}

fn supported(account: &Account) -> Result<()> {
    // HEY stores drafts through its own client with staged files; not offered here.
    if account.provider == Provider::Hey {
        Err("mail_draft_unavailable")
    } else {
        Ok(())
    }
}

fn message_id(raw: &str) -> Option<String> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    let (headers, _) = mailparse::parse_headers(&bytes).ok()?;
    use mailparse::MailHeaderMap;
    headers.get_first_value("Message-ID")
}

pub(crate) async fn draft_with(
    request: DraftRequest,
    lookup: &impl super::send::IdentityLookup,
    store: &impl DraftStore,
) -> Result<Value> {
    supported(&request.message.account)?;
    let aliases = lookup.identities(&request.message.account).await?;
    let execute = request.message.execute;
    let replace = request.replace.clone();
    let message = request.message;
    let prepared =
        tokio::task::spawn_blocking(move || super::send::prepare_draft(&message, &aliases))
            .await
            .map_err(|_| "worker_failed")??;
    let stamp =
        u64::try_from(chrono::Utc::now().timestamp_millis()).map_err(|_| "invalid_params")?;
    let payload = prepared.payload(stamp, "draft")?;
    let raw = payload["raw"].as_str().ok_or("invalid_params")?;
    let mut result = prepared.preview();
    result["draft"] = json!(true);
    result["replaces"] = json!(replace);
    if !execute {
        return Ok(result);
    }
    let saved = store
        .save(&prepared.account, raw, replace.as_deref())
        .await?;
    result["dryRun"] = json!(false);
    result["executed"] = json!(true);
    result["saved"] = json!(true);
    // The Message-ID finds the draft again (IMAP APPEND returns no identifier).
    result["messageId"] = json!(message_id(raw));
    result["draftId"] = saved
        .get("draftId")
        .or_else(|| saved.get("id"))
        .cloned()
        .unwrap_or(Value::Null);
    let warning = saved["warning"].as_str().unwrap_or("");
    if !warning.is_empty() {
        result["warning"] = json!(warning);
    }
    Ok(result)
}

pub(crate) async fn delete_with(
    request: DeleteDraftRequest,
    store: &impl DraftStore,
) -> Result<Value> {
    supported(&request.account)?;
    if request.account.provider == Provider::Jmap {
        return Err("mail_draft_unavailable");
    }
    let mut result = json!({"dryRun":!request.execute,"executed":request.execute,"accountId":request.account.id,"id":request.id});
    if request.execute {
        store.delete(&request.account, &request.id).await?;
        result["deleted"] = json!(true);
    }
    Ok(result)
}
