use super::draft::{DeleteDraftRequest, DraftRequest, DraftStore, delete_with, draft_with};
use super::send::IdentityLookup;
use super::*;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Mutex};

type Pending<'a> = Pin<Box<dyn Future<Output = Result<Value, &'static str>> + Send + 'a>>;

struct Identities;
impl IdentityLookup for Identities {
    fn identities<'a>(&'a self, _: &'a Account) -> Pending<'a> {
        Box::pin(async {
            Ok(json!([{"email":"a@example.org","displayName":"Alex","isPrimary":true}]))
        })
    }
}

#[derive(Default)]
struct Store {
    saved: Mutex<Vec<(String, Option<String>)>>,
    deleted: Mutex<Vec<String>>,
}
impl DraftStore for Store {
    fn save<'a>(&'a self, _: &'a Account, raw: &'a str, replace: Option<&'a str>) -> Pending<'a> {
        self.saved
            .lock()
            .unwrap()
            .push((raw.to_owned(), replace.map(str::to_owned)));
        Box::pin(async { Ok(json!({"saved":true,"warning":""})) })
    }
    fn delete<'a>(&'a self, _: &'a Account, id: &'a str) -> Pending<'a> {
        self.deleted.lock().unwrap().push(id.to_owned());
        Box::pin(async { Ok(json!({})) })
    }
}

fn request(provider: Provider, execute: bool) -> DraftRequest {
    DraftRequest {
        message: SendRequest {
            account: Account {
                id: "a@example.org".into(),
                provider,
            },
            from: String::new(),
            to: vec![],
            cc: vec!["copy@example.org".into()],
            bcc: vec![],
            subject: "Fwd: Offerte 99342".into(),
            body: "Beste Hanneke,\n\n---------- Forwarded message ----------\n".into(),
            attachments: vec![],
            execute,
            send_id: None,
        },
        replace: Some("7:Drafts".into()),
    }
}

#[tokio::test]
async fn a_draft_previews_by_default_and_needs_no_recipient_yet() {
    let store = Store::default();
    let preview = draft_with(request(Provider::Outlook, false), &Identities, &store)
        .await
        .unwrap();
    assert_eq!(preview["dryRun"], true);
    assert_eq!(preview["draft"], true);
    assert_eq!(preview["to"], json!([]));
    assert_eq!(preview["cc"], json!(["copy@example.org"]));
    assert!(
        store.saved.lock().unwrap().is_empty(),
        "a preview saves nothing"
    );
}

#[tokio::test]
async fn executing_saves_the_built_message_as_a_draft_replacing_the_old_one() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let store = Store::default();
    let saved = draft_with(request(Provider::Outlook, true), &Identities, &store)
        .await
        .unwrap();
    assert_eq!(
        (saved["executed"].clone(), saved["saved"].clone()),
        (json!(true), json!(true))
    );
    let calls = store.saved.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.as_deref(), Some("7:Drafts"));
    let text = String::from_utf8(URL_SAFE_NO_PAD.decode(&calls[0].0).unwrap()).unwrap();
    assert!(text.contains("Cc: copy@example.org") && text.contains("Subject: Fwd: Offerte 99342"));
    let id = saved["messageId"].as_str().unwrap();
    assert!(
        text.contains(&format!("Message-ID: {id}")),
        "the draft can be found again by its Message-ID"
    );
}

#[tokio::test]
async fn hey_drafts_and_invalid_recipients_are_refused_before_saving() {
    let store = Store::default();
    assert_eq!(
        draft_with(request(Provider::Hey, true), &Identities, &store)
            .await
            .err(),
        Some("mail_draft_unavailable")
    );
    let mut bad = request(Provider::Imap, true);
    bad.message.cc = vec!["x@example.org\r\nBcc: victim@example.org".into()];
    assert!(draft_with(bad, &Identities, &store).await.is_err());
    assert!(store.saved.lock().unwrap().is_empty());
}

#[tokio::test]
async fn deleting_a_draft_previews_then_removes_exactly_that_draft() {
    let store = Store::default();
    let account = Account {
        id: "a@example.org".into(),
        provider: Provider::Imap,
    };
    let preview = delete_with(
        DeleteDraftRequest {
            account: account.clone(),
            id: "9:Drafts".into(),
            execute: false,
        },
        &store,
    )
    .await
    .unwrap();
    assert_eq!(preview["dryRun"], true);
    assert!(store.deleted.lock().unwrap().is_empty());
    delete_with(
        DeleteDraftRequest {
            account,
            id: "9:Drafts".into(),
            execute: true,
        },
        &store,
    )
    .await
    .unwrap();
    assert_eq!(*store.deleted.lock().unwrap(), vec!["9:Drafts".to_owned()]);
}

#[test]
fn draft_params_never_carry_an_outbox_identity() {
    let params = json!({"account":"","cc":["copy@example.org"],"sendId":"x"});
    assert!(DraftRequest::try_from(&params).is_err());
}
