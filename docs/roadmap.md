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
| Copiar o mover | La elección del destino exige escribir el ID de una carpeta | Elegir carpeta por nombre o mediante arrastre |
| Permisos de automatización | Están repartidos entre el inspector de tecla/acción y la sección Automatización de Ajustes | Reunirlos en Avanzado → Automatización, ocultos salvo que se activen |
| Progreso de copias | La transferencia muestra bytes; la descarga de plugins ya tiene barra | Mostrar barra de progreso para las copias |

Fuentes de la situación actual:
[`App::advanced`](../crates/rad-editor/src/app/inspector.rs),
[`App::settings_section`](../crates/rad-editor/src/app/settings.rs) y
el manejo de `Reply::TransferProgress` en [`App`](../crates/rad-editor/src/app.rs).
El diseño se contrasta con el [checklist de UX](ux.md#checklist-para-cambios-en-la-interfaz)
y se verifica con la [política de pruebas](testing.md).
