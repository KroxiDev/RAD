//! Presentation of runtime facts; never infers hardware state from domain revision.
use serde_json::Value;
pub fn deck_label(status: &Value, es: bool) -> &'static str {
    match (status["deck_state"].as_str(), es) {
        (Some("disabled"), false) => "Deck disabled — editor available",
        (Some("disabled"), true) => "Deck deshabilitado — editor disponible",
        (Some("disconnected"), false) => "Deck disconnected — editor available",
        (Some("disconnected"), true) => "Deck desconectado — editor disponible",
        (Some("connected"), false) => "Deck connected",
        (Some("connected"), true) => "Deck conectado",
        (_, false) => "Deck state unknown",
        (_, true) => "Estado del deck desconocido",
    }
}
pub fn recovery_notice(status: &Value, es: bool) -> Option<&'static str> {
    (status["recovered_previous"] == true).then_some(if es {
        "Se recuperó el último respaldo válido. Pueden faltar cambios recientes."
    } else {
        "Recovered the last valid backup. Recent changes may be missing."
    })
}
/// A sentence for a known error code. Unknown codes pass through, never hidden.
pub fn describe_error(code: &str, es: bool) -> &str {
    let (en, es_text) = match code {
        "unsupported_version" => (
            "Update RAD to open this backup",
            "Actualiza RAD para abrir esta copia",
        ),
        "plugin_failed" => ("Plugin failed", "Falló el plugin"),
        "plugin_limit" => (
            "Maximum of 32 plugins reached",
            "Se alcanzó el máximo de 32 plugins",
        ),
        "protocol" => (
            "Plugin sent an invalid protocol message",
            "El plugin envió un mensaje de protocolo inválido",
        ),
        "pipe" => (
            "Plugin communication failed",
            "Falló la comunicación con el plugin",
        ),
        "exited" => ("Plugin process exited", "El proceso del plugin terminó"),
        "backpressure" => (
            "Plugin communication queue is full",
            "La cola de comunicación del plugin está llena",
        ),
        "plugin_in_use" => (
            "Plugin in use — retry manually later",
            "Plugin en uso — vuelve a intentarlo manualmente",
        ),
        "plugin_residual" => (
            "Plugin has residual resources",
            "Plugin con recursos residuales",
        ),
        "publication_pending" => (
            "Another publication is pending",
            "Hay otra publicación pendiente",
        ),
        "incompatible_plugin" => (
            "Incompatible plugin — configuration preserved",
            "Plugin incompatible — configuración conservada",
        ),
        "invalid_source" => (
            "Use a public GitHub repository or tagged release",
            "Usa un repositorio público GitHub o release con etiqueta",
        ),
        "invalid_package" => ("Invalid plugin package", "Paquete de plugin inválido"),
        "plugin_already_installed" => (
            "Plugin already installed — use Update",
            "El plugin ya está instalado — usa Actualizar",
        ),
        "plugin_mismatch" => (
            "That package belongs to a different plugin",
            "El paquete no corresponde a este plugin",
        ),
        "plugin_same_version" => (
            "That version is already selected — nothing to update",
            "Esa versión ya está seleccionada — no hay nada que actualizar",
        ),
        "network" => ("Plugin download failed", "Falló la descarga del plugin"),
        "deadline" => (
            "Plugin operation timed out",
            "La operación del plugin agotó su plazo",
        ),
        "io" => (
            "Plugin storage operation failed",
            "Falló el almacenamiento del plugin",
        ),
        "recovery_required" => (
            "Plugin selection uncertain — restart RAD before continuing",
            "Selección de plugin incierta — Reinicia RAD antes de continuar",
        ),
        "invalid_configuration" => (
            "That change is not valid for this key",
            "Ese cambio no es válido para esta tecla",
        ),
        "invalid_request" => ("That request is not valid", "Esa solicitud no es válida"),
        "operation_not_allowed" => ("That is not allowed here", "Eso no está permitido aquí"),
        "permission_denied" => ("Permission denied", "Permiso denegado"),
        "not_found" => ("It no longer exists", "Ya no existe"),
        "busy" => (
            "Busy: that key is still running",
            "Ocupado: esa tecla sigue en ejecución",
        ),
        "resource_exhausted" => ("Limit reached", "Límite alcanzado"),
        "storage_error" => ("Could not save to disk", "No se pudo guardar en disco"),
        "storage_uncertain" => (
            "Unsure whether it was saved; check before continuing",
            "No se sabe si se guardó; compruébalo antes de seguir",
        ),
        "deadline_exceeded" => (
            "RAD took too long to answer",
            "RAD tardó demasiado en responder",
        ),
        "runtime_unavailable" => ("RAD is not running", "RAD no está en marcha"),
        "executor_unavailable" | "service_unavailable" => (
            "That action is not available right now",
            "Esa acción no está disponible ahora",
        ),
        "options_timeout" => (
            "The plugin did not answer in time; try again",
            "El plugin no respondió a tiempo; vuelve a intentarlo",
        ),
        "cursor_expired" => (
            "The list changed; search again",
            "La lista cambió; busca de nuevo",
        ),
        "file_error" => (
            "Could not read or write that file",
            "No se pudo leer o escribir ese archivo",
        ),
        "result_expired" => (
            "That result is no longer available",
            "Ese resultado ya no está disponible",
        ),
        _ => return code,
    };
    if es { es_text } else { en }
}

/// Keeps the wire code for details while explaining capacity failures with a way out.
pub fn describe_failure(error: &crate::session::Error, es: bool) -> &str {
    if error.code != "resource_exhausted" {
        return describe_error(&error.code, es);
    }
    let (en, translated) = match error.cause.as_deref() {
        Some("entity_limit") => (
            "No more buttons can be created: maximum reached — remove unused items",
            "No se pueden crear más botones: se alcanzó el máximo — elimina elementos que no uses",
        ),
        Some("identity_counter_exhausted") => (
            "No new identities are available — export a backup and contact support",
            "No quedan identidades nuevas — exporta una copia y contacta con soporte",
        ),
        Some("document_limit") => (
            "Configuration is too large — shorten texts or remove unused items",
            "La configuración es demasiado grande — acorta textos o elimina elementos que no uses",
        ),
        Some("storage_capacity") => (
            "Not enough storage to save — free disk space and retry",
            "No hay espacio para guardar — libera espacio en disco y reintenta",
        ),
        _ => return describe_error(&error.code, es),
    };
    if es { translated } else { en }
}
