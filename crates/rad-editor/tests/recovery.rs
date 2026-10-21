use rad_editor::recovery::{Action, Screen};
use serde_json::json;

fn screen(cause: &str, actions: &[&str]) -> Screen {
    Screen::from_value(&json!({
        "cause": cause, "actions": actions,
        "detail": {"code": "corrupt", "counters_read": false}
    }))
}

#[test]
fn the_view_follows_the_cause_and_offers_only_what_the_runtime_lists() {
    let with_restore = screen("unreadable", &["restore", "reset", "details"]);
    assert_eq!(
        with_restore.actions,
        [Action::Restore, Action::Reset, Action::Details]
    );
    let without = screen("unreadable", &["reset", "details"]);
    assert!(!without.offers(Action::Restore) && without.offers(Action::Reset));
    for cause in ["newer_version", "unsafe_folder"] {
        let only_details = screen(cause, &["details"]);
        assert_eq!(only_details.actions, [Action::Details], "{cause}");
    }
    // An action this editor does not know is not guessed.
    assert_eq!(
        screen("unreadable", &["future", "details"]).actions,
        [Action::Details]
    );
}

#[test]
fn every_cause_and_action_reads_plainly_in_both_languages() {
    for (cause, en, es) in [
        (
            "unreadable",
            "Your configuration could not be opened",
            "No se pudo abrir tu configuración",
        ),
        (
            "newer_version",
            "Update RAD to open this configuration",
            "Actualiza RAD para abrir esta configuración",
        ),
        (
            "unsafe_folder",
            "RAD cannot use its data folder",
            "RAD no puede usar su carpeta de datos",
        ),
        ("future", "Something went wrong", "Algo salió mal"),
    ] {
        let screen = screen(cause, &["details"]);
        assert_eq!(screen.title(false), en);
        assert_eq!(screen.title(true), es);
    }
    for (action, en, es) in [
        (Action::Restore, "Restore a copy", "Restaurar una copia"),
        (Action::Reset, "Start over", "Empezar de nuevo"),
        (Action::Details, "View details", "Ver detalles"),
    ] {
        assert_eq!((action.label(false), action.label(true)), (en, es));
    }
    let read = Screen::from_value(&json!({"cause":"unreadable","actions":[],
        "detail":{"code":"invalid","counters_read":true}}));
    assert_eq!(read.details(false), "invalid\nIdentity counters were read");
    assert_eq!(
        screen("unreadable", &[]).details(true),
        "corrupt\nNo se pudieron leer los contadores de identidad"
    );
}
