# Modelo de dominio

Guía de relaciones y escenarios para quienes cambian RAD. Usa el vocabulario de
[CONTEXT.md](../CONTEXT.md) y conecta el modelo con código y pruebas existentes;
no introduce comportamiento nuevo. Los dueños y dependencias están en
[Arquitectura](architecture.md), la representación en [Formatos](formats.md) y
el contrato de observación en [API de clientes](client-api.md).

## Configuración, posiciones y entidades

```text
Configuración del deck
├── rejilla y ajustes del deck
├── permisos globales y ajustes no secretos de plugins
└── árbol de carpetas, con una carpeta principal
    └── posiciones vacías u ocupadas por un botón
        ├── nombre, apariencia, visual y permisos
        └── plan: acciones ordenadas, modo y gesto
```

La **posición es una relación**, no la identidad del botón. Mover un botón cambia
su ubicación; duplicarlo crea otro botón y otras acciones. Dos botones con el mismo
nombre siguen siendo entidades distintas. La tecla física se interpreta contra
la carpeta activa: no es un alias permanente de un botón.

La **carpeta no tiene nombre ni permisos propios**. El nombre presentado de una
carpeta hija procede de su botón de entrada, situado en el padre. Su botón de
retorno vive dentro de la hija. Entrada y retorno son botones protegidos de
navegación, no planes ordinarios a los que se agregan acciones. El árbol tiene una
sola raíz, sin ciclos; cada botón ocupa exactamente una posición. La especificación
completa de estas relaciones está en
[Formatos → Relaciones del árbol](formats.md#relaciones-del-árbol).

**Escenario límite:** mover el botón de entrada de una carpeta también cambia la
relación de esa carpeta con su padre; moverlo dentro de su propia descendencia
crearía un ciclo y se rechaza. Intercambiar posiciones ocupadas no autoriza a
desplazar el retorno a otra carpeta.

Fuentes: `FolderData`, `ButtonData`, `ActionData` y `Edit` en
[configuration.rs](../crates/rad-core/src/configuration.rs),
[ediciones](../crates/rad-core/src/configuration/edits.rs) y
[validación](../crates/rad-core/src/configuration/validation.rs).
Pruebas de referencia:
[configuration_invariants.rs](../crates/rad-core/tests/configuration_invariants.rs).

## Identidad, historial y copias

`EntityIdentity` distingue la procedencia de una entidad; `ShortId` es su referencia
abreviada por clase, como `b12`. El nombre visible y la posición no sustituyen a
ninguno. Deshacer restaura contenido, **no retrocede los contadores de asignación**.
Una entidad retirada puede seguir siendo necesaria para el historial o una copia
en curso; retirarla del contenido vigente no autoriza a reutilizar su identificador
para otra entidad.

**Escenario límite:** un destino que creó un botón y después lo deshizo puede verse
vacío, pero no es una instalación nunca editada. Al restaurar una copia extranjera
debe reservar ambos rangos y reasignar por encima de ellos, no adoptar los IDs como
si fuera nuevo. La restauración de la misma instalación, la adopción inicial y la
excepción estrecha de la raíz vacía tienen condiciones distintas en
[Política de restauración](formats.md#política-de-restauración).

Restaurar reemplaza la configuración y se puede deshacer; no fusiona decks, no
ejecuta acciones y no instala plugins. Una referencia a un plugin ausente conserva
configuración, pero no acredita que haya un ejecutable disponible.

Fuentes: [identity.rs](../crates/rad-core/src/identity.rs),
[restauración](../crates/rad-core/src/configuration/restore.rs) y
[ADR 0002](adr/0002-identidades-sobre-lo-vigente.md).
Pruebas de referencia:
[identity_counters.rs](../crates/rad-core/tests/identity_counters.rs) y
[portable_restore.rs](../crates/rad-core/tests/portable_restore.rs), en particular
`edited_then_undone_target_remaps_above_both_ranges_and_can_undo`.

## Ajustes, parámetros y secretos

Los ajustes no secretos de un plugin pertenecen a la configuración portable del
deck. No son una cuarta categoría junto a deck, máquina y editor, ni se confunden
con los parámetros de una acción o de un visual particular. Un cambio de idioma
es una preferencia del editor; usar el Stream Deck con RAD es un ajuste de la máquina;
cambiar brillo es configuración del deck. Para asignar un dato nuevo a su dueño,
usar [ADR 0005](adr/0005-tres-tipos-de-ajustes.md).

**Escenario límite:** al restaurar una copia en otro equipo se recuperan los ajustes
no secretos de OBS, pero no su contraseña ni su ejecutable. El permiso de restaurar
no concede confianza a código nativo ni convierte una referencia de distribución
en autorización para descargarlo. Ver
[Copias RADPKG](formats.md#copias-radpkg) y
[Confianza en plugins](adr/0009-confianza-en-plugins.md).

Prueba de referencia:
`imported_navigation_and_plugin_settings_remap_together` en
[portable_restore.rs](../crates/rad-core/tests/portable_restore.rs).

## Permisos y origen

Los permisos de automatización distinguen consultar, modificar y ejecutar. Los
globales son la base del botón; la acción hereda del botón. Un conjunto local
propio **reemplaza el conjunto heredado**, no lo mezcla bit a bit; modificar o
ejecutar requieren consultar. La carpeta no agrega otra capa de permisos.

`Origin` clasifica las peticiones de editor y automatización. `Trigger` distingue
la activación física de la ejecución pedida por editor o automatización. Ninguno
es el origen de distribución de un plugin, y un campo del cliente no puede
concederse la procedencia del editor.

**Escenario límite:** una acción con permiso propio de consulta puede ser visible
aunque su botón esté oculto. Eso no autoriza a revelar el botón ni su identidad
como padre. La consulta de resultados usa permisos actuales, no los permisos que
se capturaron al admitir la ejecución.

Fuentes: [permissions.rs](../crates/rad-core/src/permissions.rs),
[consultas de Authority](../crates/rad-authority/src/queries.rs) y
[API de clientes](client-api.md#transporte-y-compatibilidad).
Pruebas de referencia: [permissions.rs](../crates/rad-core/tests/permissions.rs) y
[queries.rs](../crates/rad-authority/tests/queries.rs), especialmente
`child_override_visible_under_hidden_parent_and_hidden_get_is_not_found` y
`result_filters_current_c_not_snapshot_e_and_child_does_not_require_parent_c`.

## Gesto, admisión y ejecución

La entrada física produce candidatos; **admitir** valida el plan o la acción
pedida y captura su configuración. Una ejecución ya admitida no cambia de
parámetros porque se edite el botón después. El umbral físico se captura al
presionar, mientras que los parámetros se toman al admitir: esos momentos no son
intercambiables. Una petición de ejecución aceptada tampoco equivale a una acción
externa ya terminada.

El gesto se elige para el plan completo. Mantener exige acciones externas capaces
de recibir inicio y fin; CLI y «Probar» solicitan una ejecución ordinaria, no
simulan presionar y soltar. La semántica completa está en
[ADR 0015](adr/0015-gesto-del-plan-y-documento-v3.md).

**Escenario límite:** se mantiene una tecla y se navega a otra carpeta antes de
soltarla. La navegación termina el mantener en ese momento: el fin va a los inicios
enviados del plan original, con su captura original, nunca al botón que ahora
ocupa esa posición, y soltar después no produce otro fin. Los pasos que aún no
habían enviado su inicio se cancelan. El reposo y la desconexión también terminan
el mantener; despertar o reconectar no repiten los inicios.

Fuentes: `Input` en [input.rs](../crates/rad-core/src/input.rs), `Snapshot` y
`Engine::admit` en [execution.rs](../crates/rad-core/src/execution.rs), y
[Authority](../crates/rad-authority/src/lib.rs).
Pruebas de referencia:
`hold_ignores_press_threshold_and_ends_original_binding_on_layout_change` y
`hold_cancellation_sends_one_end_and_wake_or_reconnect_never_replays_start` en
[input.rs](../crates/rad-core/tests/input.rs);
`release_cancels_future_sequential_starts_and_retains_original_snapshot_until_end` y
`native_sleep_ends_original_hold_and_cancels_its_unsent_steps` en
[gestures.rs](../crates/rad-authority/tests/gestures.rs); y
`physical_input_captures_threshold_but_parameters_only_at_admission` en
[runtime.rs](../crates/rad-authority/tests/runtime.rs).

## Reposo, visuales y estado observado

Son tres distinciones que evitan inferir efectos a partir de lo mostrado:

- **Reposo no es apagado de RAD.** La acción nativa `Sleep` («Suspender Stream
  Deck» en la interfaz) activa el mismo reposo que la inactividad, incluso con
  reposo automático desactivado. Conserva carpeta y brillo configurado; no
  suspende el equipo ni cancela los pasos restantes de su plan.
  La presión que despierta se consume hasta soltar, sin ejecutar su botón.
  Contrato: [ADR 0017](adr/0017-accion-nativa-sleep-y-documento-v4.md).
- **Visual no es acción.** Su tipo de visual puede pertenecer a un plugin distinto
  del de las acciones. Un visual no disponible no decide por sí solo si una
  ejecución se admite, y un proceso de plugin vivo no demuestra que su fuente siga
  observada. La vigencia vence si no se renueva desde el camino que observa la
  fuente.
  Contrato: [ADR 0008](adr/0008-frescura-declarada.md).
- **Revisión visible no es revisión durable.** Navegación y ejecución pueden
  cambiar una vista sin guardar configuración. La revisión de un aviso invalida
  una consulta: no es un historial reproducible. Ante un resultado `Unknown`, el
  cliente relee; no repite automáticamente una acción.
  Contrato: [API de clientes](client-api.md#suscripción-sin-sondeo).

Pruebas de referencia:
`sleep_is_an_ordered_native_plan_step_and_does_not_stop_the_remaining_plan` y
`physical_sleep_preserves_folder_and_brightness_and_consumes_the_whole_wake_press`
en [runtime.rs](../crates/rad-authority/tests/runtime.rs);
`editor_view_revision_tracks_execution_and_selection_not_only_store_revision` en
[queries.rs](../crates/rad-authority/tests/queries.rs); y
[freshness_v1.rs](../crates/rad-plugin-host/tests/freshness_v1.rs).

Estas referencias identifican contratos existentes, no acreditan que sus pruebas
se hayan ejecutado al leer o editar este documento. Para validar un cambio, seguir
[Pruebas y validación](testing.md).
