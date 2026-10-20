use rad_editor::status::deck_label;
use serde_json::json;
#[test]
fn deck_label_uses_fresh_runtime_fact_without_revision_change_in_both_languages() {
    for (state, en, es) in [
        (
            "disabled",
            "Deck disabled — editor available",
            "Deck deshabilitado — editor disponible",
        ),
        (
            "disconnected",
            "Deck disconnected — editor available",
            "Deck desconectado — editor disponible",
        ),
        ("connected", "Deck connected", "Deck conectado"),
        (
            "inaccessible",
            "Deck inaccessible — editor available",
            "Deck inaccesible — editor disponible",
        ),
    ] {
        let status = json!({"deck_state":state,"view_revision":"7"});
        assert_eq!(deck_label(&status, false), en);
        assert_eq!(deck_label(&status, true), es);
    }
    assert_eq!(deck_label(&json!({}), false), "Deck state unknown");
}
#[test]
fn recovery_notice_requires_runtime_confirmation_and_names_last_valid_backup() {
    use rad_editor::status::recovery_notice;
    let recovered = json!({"recovered_previous":true,"view_revision":"7"});
    assert_eq!(
        recovery_notice(&recovered, false),
        Some("Recovered the last valid backup. Recent changes may be missing.")
    );
    assert_eq!(
        recovery_notice(&recovered, true),
        Some("Se recuperó el último respaldo válido. Pueden faltar cambios recientes.")
    );
    assert_eq!(
        recovery_notice(
            &json!({"recovered_previous":false,"view_revision":"7"}),
            false
        ),
        None
    );
    assert_eq!(recovery_notice(&json!({}), true), None);
}
#[test]
fn error_codes_read_as_sentences_and_unknown_codes_have_plain_fallback() {
    use rad_editor::status::describe_error;
    assert_eq!(
        describe_error("invalid_configuration", false),
        "That change is not valid for this key"
    );
    assert_eq!(
        describe_error("invalid_configuration", true),
        "Ese cambio no es válido para esta tecla"
    );
    assert_eq!(
        describe_error("busy", true),
        "Ocupado: esa tecla sigue en ejecución"
    );
    assert_eq!(
        describe_error("plugin_already_installed", true),
        "El plugin ya está instalado — usa Actualizar"
    );
    assert_eq!(
        describe_error("plugin_mismatch", true),
        "El paquete no corresponde a este plugin"
    );
    assert_eq!(
        describe_error("plugin_same_version", true),
        "Esa versión ya está seleccionada — no hay nada que actualizar"
    );
    assert_eq!(
        describe_error("some_future_code", false),
        "Something went wrong"
    );
    assert_eq!(describe_error("some_future_code", true), "Algo salió mal");
}

#[test]
fn a_plugin_that_does_not_answer_a_list_in_time_reads_as_a_sentence_in_both_languages() {
    use rad_editor::status::describe_error;
    assert_eq!(
        describe_error("options_timeout", false),
        "The plugin did not answer in time; try again"
    );
    assert_eq!(
        describe_error("options_timeout", true),
        "El plugin no respondió a tiempo; vuelve a intentarlo"
    );
}

#[test]
fn future_backup_requires_updating_rad_in_both_languages() {
    assert_eq!(
        rad_editor::status::describe_error("unsupported_version", false),
        "Update RAD to open this backup"
    );
    assert_eq!(
        rad_editor::status::describe_error("unsupported_version", true),
        "Actualiza RAD para abrir esta copia"
    );
}
