# 0019 · Brightness persistente y documento v5

**Estado:** aceptada, 2026-10-25.

El usuario pide una acción nativa Brightness, en el grupo RAD, con una única
elección: subir o bajar diez puntos porcentuales. Confirma que el brillo resultante
debe guardarse y reflejarse en Ajustes. El mínimo configurable pasa a 10%, con
máximo 100%; el reposo del deck sigue siendo una función distinta.

## Ejecución y publicación

La dirección se captura con la admisión ordinaria del plan; el cálculo usa el
brillo publicado vigente cuando le corresponde ejecutar, no un porcentaje
capturado al configurar la tecla. No se redondea a múltiplos de diez: 55% pasa a
65% o 45%; los extremos saturan, por ejemplo 95% a 100% y 15% a 10%.

Authority conserva el único dueño del ajuste y utiliza la publicación durable
existente, sin tickets de cliente ficticios ni una escritura paralela. El paso
solo termina satisfactoriamente después de `Applied`; `NotApplied` lo falla y un
resultado incierto mantiene su correlación para reconciliar, sin repetir el
delta. En el límite, una acción satisfactoria no cambia la revisión ni escribe.
Las esperas están acotadas por los trabajos admitidos y respetan su vencimiento;
un trabajo vencido en cola no aplica un ajuste tardío. Una transacción ya iniciada
se resuelve sin reanudar los pasos de una ejecución vencida.

No cambia navegación, reposo ni otros ajustes. Conserva el orden del plan,
permisos y snapshot de la acción, y admite pulsar y pulsación larga, no mantener.
La proyección de Ajustes se actualiza sin descartar borradores reales. El sol
nativo es solo presentación, como la luna de Sleep: no añade estado ni medios.

## Persistencia y compatibilidad

La variante `{"brightness":"increase"}` o `{"brightness":"decrease"}` exige
documento **v5**, siguiendo la regla de [ADR 0017](0017-accion-nativa-sleep-y-documento-v4.md).
Los importadores cerrados de v1 a v4 conservan las acciones anteriores y no
aceptan Brightness bajo una etiqueta anterior. Los porcentajes históricos válidos
de 0 a 9 se convierten a 10; el documento actual y las ediciones exigen 10–100.
No se convierte un valor inválido mayor de 100 en una configuración válida.

Store SQLite y RADPKG siguen en envelope v2. La apertura conserva el original de
un Store con documentos v4 como `.document-v4-backup`, sin sobrescribir evidencia,
y convierte atómicamente las revisiones válidas. Los respaldos e importadores
anteriores siguen disponibles; no se pierden identidades, medios, historial ni
evidencia corrupta. El golden v4 sintético se conserva del escritor anterior.
Un lector anterior detecta una versión posterior y requiere actualizar RAD, no
restablecer la configuración ni hacer downgrade.

La API local y CLI conservan framing v1. El tipo `brightness`, sus parámetros
`{"direction":"increase"}` o `{"direction":"decrease"}` y el icono nativo son variantes nuevas:
un cliente cerrado anterior debe actualizarse para configurarlas o mostrarlas.
Editor y runtime se distribuyen juntos en el paquete; no cambia el contrato de
plugins ni se introduce una autoridad de configuración en el editor.
