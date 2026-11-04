# Pendientes de RAD

Trabajo pendiente respecto de la [arquitectura](architecture.md) y la
[norma de UX](ux.md). Cada ítem requiere un encargo explícito; esta lista no fija
fechas, prioridad de ejecución ni compromisos nuevos.

## UX pendiente

**Aplazada por decisión del usuario.** No se inicia sin un nuevo encargo. El
pendiente es de presentación: las interfaces actuales siguen siendo la base y
no se crea otra autoridad en el editor.

| Área | Situación actual | Cambio pendiente |
|---|---|---|
| Identificadores | Los IDs se muestran y copian en «Avanzado» del inspector de cada tecla | Reunirlos en Ajustes → Avanzado, conforme a S2 |
| Mover carpetas | Copiar y pegar usa Ctrl+C / Ctrl+V sin IDs; mover una carpeta todavía exige escribir el ID de destino | Elegir carpeta por nombre o mediante arrastre para mover carpetas |
| Permisos de automatización | Están repartidos entre el inspector de tecla/acción y la sección Automatización de Ajustes | Reunirlos en Avanzado → Automatización, ocultos salvo que se activen |
| Progreso de copias | La transferencia muestra bytes; la descarga de plugins ya tiene barra | Mostrar barra de progreso para las copias |

Fuentes de la situación actual:
[`App::advanced`](../crates/rad-editor/src/app/inspector.rs),
[`App::settings_section`](../crates/rad-editor/src/app/settings.rs) y
el manejo de `Reply::TransferProgress` en [`App::pump_replies`](../crates/rad-editor/src/app/replies.rs).
El diseño se contrasta con el [checklist de UX](ux.md#checklist-para-cambios-en-la-interfaz)
y se verifica con la [política de pruebas](testing.md).

## Distribución y actualizaciones

Implementada según [ADR 0018](adr/0018-distribucion-y-actualizacion-de-la-instalacion.md)
y el [procedimiento de empaquetado](../packaging/README.md). La actualización
integrada entre releases estables ya se comprobó con systemd, polkit y pacman
reales. Pendiente de cierre:

- Validar en una sesión Omarchy aislada lo que el uso normal no cubre: cancelación,
  fallos antes y después de la transacción, desinstalación y reinstalación con
  datos sintéticos. Los dobles no certifican esa integración.
- Congelar y validar un snapshot de construcción Omarchy 4.0.6; el número de
  distribución no congela sus bibliotecas rolling.

No introducir CI, AUR, canales beta, actualización de plugins o rollback
automático por inferencia.
