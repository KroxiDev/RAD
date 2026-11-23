# 0024 · Lecturas anticipadas y borradores del editor

**Estado** — Aceptada, 2026-11-23.

Se registra la decisión vigente que desarrolla las vistas coherentes y los avisos de
[ADR 0010](0010-api-de-clientes.md). Mientras llega la respuesta de un worker, el
usuario puede cambiar de tecla, seguir escribiendo o ejecutar otro comando. La
respuesta anterior debe corresponder todavía a la selección y a la revisión que
se muestran.

## Mantener una vista coherente

El editor solicita juntos el tablero nuevo y el inspector de la tecla seleccionada.
Mientras llegan, conserva el par anterior. Si las revisiones difieren, vuelve a
leer ambos. En los turnos libres adelanta los inspectores de las demás teclas y
las carpetas a una pulsación, empezando por la carpeta superior. Aprovecha una
lectura anticipada solo mientras siga siendo válida para el tablero. Un cambio
de plugins puede invalidar los inspectores aunque siga vigente su revisión.

El editor y el runtime dan prioridad a las imágenes de la carpeta visible.
Dedican un presupuesto adicional a las carpetas cercanas y a visitas anteriores.
Al necesitar espacio, sale primero una visita anterior y, a igual prioridad,
la imagen pedida hace más tiempo. El editor adelanta imágenes en lotes pequeños
para que el próximo comando espere poco. Una lectura fallida vuelve a admitirse
cuando cambia el tablero. Una imagen que llega para una tecla visible aparece
gradualmente. Si ya estaba en caché, se puede mostrar al seleccionar la tecla.
Las miniaturas de aplicaciones, en cambio, caducan tras 30 segundos fuera de
pantalla. Una fila sigue visible durante los intervalos en que egui duerme.
[Lecturas anticipadas](../../crates/rad-editor/src/app/ahead.rs),
[imágenes del editor](../../crates/rad-editor/src/app/pictures.rs) e
[imágenes del runtime](../../crates/rad-runtime/src/pictures.rs).

## Conservar lo que el usuario sigue editando

Cada Guardar queda asociado al contenido enviado y a su respuesta. Al releer,
los campos intactos siguen al runtime y las ediciones posteriores siguen en el
borrador. Si se perdió la respuesta, el runtime puede conservar el valor
anterior o haber aplicado el enviado. El editor contrasta ambos al recuperar
el estado. Una duración se contrasta por su valor numérico para aceptar la
confirmación aunque cambien los espacios o los ceros iniciales. Deshacer o
rehacer es una petición explícita que permite reemplazar el borrador al releer.

Las transferencias pertenecen a la conexión que las abrió. Al perderla, el editor
invalida las preparaciones de esa sesión. Una receta local puede generar de nuevo
su imagen. La elección de aplicación e imagen también captura la tecla de destino
para que una selección posterior no cambie lo que se guarda. El portapapeles de
teclas conserva la referencia al origen. Un sondeo anterior al pegado no consume
el destino pendiente. [Pruebas de recarga](../../crates/rad-editor/src/app/reload_tests.rs)
y [gestión de imágenes](../../crates/rad-editor/src/app/icon_library.rs).

## Dar prioridad a los comandos

El worker principal admite un trabajo en curso y una orden de usuario en espera.
Las lecturas en segundo plano se turnan cuando queda sitio. Así los avisos
continuos dejan avanzar imágenes y transferencias, y un campo de texto conserva
el foco mientras se lee. El worker despierta la GUI al terminar. Si un handler
corta el turno con trabajo listo, el editor programa otro dibujo. Cuando muere
el hilo se distingue lo que pudo haber llegado de lo que seguía en cola. La
recuperación tiene un número acotado de intentos consecutivos y el usuario puede
pedir otro. [Worker](../../crates/rad-editor/src/worker.rs) y
[pruebas de pérdida del worker](../../crates/rad-editor/src/app/worker_loss_tests.rs).

Una pregunta de opciones dinámicas puede ocupar al plugin hasta su plazo.
Tiene una conexión propia para que Guardar y las demás lecturas sigan disponibles.
Cada formulario mantiene una pregunta pendiente y acepta solo la respuesta de
su contexto. El worker renueva su conexión inactiva al recibir trabajo nuevo y
cierra el socket anterior antes de abrir el siguiente para respetar la cuota de
conexiones del runtime. [Opciones](../../crates/rad-editor/src/options.rs) y
[conexiones de los workers](../../crates/rad-editor/src/native.rs).

El worker copia el callback de redibujado bajo el mutex y lo invoca después de
liberarlo para que un panic en ese callback no envenene el registro. El runtime
repinta la carpeta visible cuando termina de cargar su conjunto de imágenes.
Precargar las imágenes de otra carpeta deja intacto el repintado de la actual.
