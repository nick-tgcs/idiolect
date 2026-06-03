use idiolectd::daemon::FixtureDaemon;

#[test]
fn fake_dictation_loop_corrects_and_commits_one_session() {
    let mut daemon = FixtureDaemon::new_for_tests("restart traffic");

    daemon.begin_fake_dictation().unwrap();
    daemon.correct("restart Traefik").unwrap();
    daemon.commit().unwrap();

    assert_eq!(
        daemon.input_events(),
        [
            "show_preedit:restart traffic",
            "update_preedit:restart Traefik",
            "commit:restart Traefik"
        ]
    );
    assert_eq!(daemon.training_candidate_count(), 1);
}

#[test]
fn fake_dictation_loop_duplicate_commit_is_idempotent() {
    let mut daemon = FixtureDaemon::new_for_tests("restart traffic");

    daemon.begin_fake_dictation().unwrap();
    daemon.correct("restart Traefik").unwrap();
    daemon.commit().unwrap();
    daemon.commit().unwrap();

    assert_eq!(
        daemon.input_events(),
        [
            "show_preedit:restart traffic",
            "update_preedit:restart Traefik",
            "commit:restart Traefik"
        ]
    );
    assert_eq!(daemon.training_candidate_count(), 1);
}

#[test]
fn fake_dictation_loop_cancel_clears_preedit_without_candidate() {
    let mut daemon = FixtureDaemon::new_for_tests("open notes");

    daemon.begin_fake_dictation().unwrap();
    daemon.cancel().unwrap();

    assert_eq!(
        daemon.input_events(),
        ["show_preedit:open notes", "cancel_preedit"]
    );
    assert_eq!(daemon.training_candidate_count(), 0);
}
