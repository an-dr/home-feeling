use bones_messages::ui::Event;

use crate::{Capability, Consent, Manifest, PromptAnswer, Status};

fn capabilities() -> Vec<Capability> {
    vec![
        Capability::new("windows", "Move and resize windows", &["desktop/windows/*"]),
        Capability::new("hotkeys", "Global hotkeys", &["desktop/hotkeys/*"]),
    ]
}

fn manifest(requests: &[&str]) -> Manifest {
    Manifest::parse(&format!(
        "requests = [{}]",
        requests
            .iter()
            .map(|r| format!("\"{r}\""))
            .collect::<Vec<_>>()
            .join(",")
    ))
    .unwrap()
}

#[test]
fn a_new_request_is_pending_and_denied_everything() {
    let dir = tempfile::tempdir().unwrap();
    let consent = Consent::open(dir.path().join("consent.json"), capabilities());
    let wants = manifest(&["windows"]);
    assert_eq!(consent.status("t", &wants), Status::Pending);
    assert_eq!(
        consent.deny_list("t", &wants),
        vec!["desktop/windows/*", "desktop/hotkeys/*"]
    );
}

#[test]
fn an_answer_is_remembered_across_opens_until_the_request_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("consent.json");
    let wants = manifest(&["windows", "hotkeys"]);
    let mut consent = Consent::open(&path, capabilities());
    consent.answer("t", &wants, &["windows".into()]).unwrap();

    let consent = Consent::open(&path, capabilities());
    assert_eq!(
        consent.status("t", &wants),
        Status::Answered {
            granted: vec!["windows".into()]
        }
    );
    assert_eq!(consent.deny_list("t", &wants), vec!["desktop/hotkeys/*"]);
    assert_eq!(
        consent.status("t", &manifest(&["windows"])),
        Status::Pending
    );
}

#[test]
fn only_requested_ids_can_be_granted() {
    let dir = tempfile::tempdir().unwrap();
    let mut consent = Consent::open(dir.path().join("c.json"), capabilities());
    let wants = manifest(&["windows"]);
    consent
        .answer("t", &wants, &["windows".into(), "hotkeys".into()])
        .unwrap();
    assert_eq!(consent.deny_list("t", &wants), vec!["desktop/hotkeys/*"]);
}

#[test]
fn a_guest_requesting_nothing_needs_no_answer() {
    let dir = tempfile::tempdir().unwrap();
    let consent = Consent::open(dir.path().join("c.json"), capabilities());
    assert_eq!(
        consent.status("t", &Manifest::default()),
        Status::Answered { granted: vec![] }
    );
}

#[test]
fn unknown_requests_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let consent = Consent::open(dir.path().join("c.json"), capabilities());
    assert_eq!(
        consent.unknown(&manifest(&["windows", "camera"])),
        vec!["camera"]
    );
}

#[test]
fn an_unreadable_store_is_set_aside() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.json");
    std::fs::write(&path, "not json").unwrap();
    let consent = Consent::open(&path, capabilities());
    assert_eq!(
        consent.status("t", &manifest(&["windows"])),
        Status::Pending
    );
    assert!(dir.path().join("c.unreadable.json").exists());
}

#[test]
fn prompt_clicks_map_to_answers() {
    assert_eq!(
        PromptAnswer::from_event(10, &Event::Clicked { id: 11 }),
        Some(PromptAnswer::Allow)
    );
    assert_eq!(
        PromptAnswer::from_event(10, &Event::Clicked { id: 12 }),
        Some(PromptAnswer::Refuse)
    );
    assert_eq!(
        PromptAnswer::from_event(10, &Event::Clicked { id: 13 }),
        None
    );
}
