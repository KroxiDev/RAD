# 0016 · Presencia de escritorio sin editor residente

**Estado:** aceptada, 2026-10-21 (entregas E1, E2 y E3 de la fase 3.5 del
[plan original](https://github.com/KroxiDev/RAD/blob/30eb0e831c5873df944a6cf1ec7ea37a5fd39191/docs/roadmap.md)).

RAD necesita identidad de aplicación y una bandeja mientras el runtime funciona,
incluso tras cerrar el editor o después del autoinicio. El runtime sigue siendo
la única autoridad y no incorpora escritorio, GIO ni GPU.

- `packaging/rad.png` es la única fuente del ícono de ventana, entrada de menú y
  bandeja. El app_id estable es `io.github.KroxiDev.RAD`.
- Al abrir, el editor instala la entrada `.desktop` y el ícono de 256 px en
  `$XDG_DATA_HOME` (o `~/.local/share`) con el criterio de ADR 0012: los escribe
  cuando faltan o cambiaron, con `Exec=` al editor en uso. Un fallo no impide abrirlo.
- Un cliente separado, `rad-tray`, publica StatusNotifierItem y DBusMenu en el bus
  D-Bus de sesión con GIO. Se registra cuando aparece el watcher, también si llega
  tarde o se reinicia. Sin watcher, no muestra nada y el runtime sigue funcionando.
  El escritorio decide dónde ubicarlo: RAD no modifica Waybar ni su grupo de iconos.
- El editor instala la unidad auxiliar si el binario hermano está presente, junto
  a la del runtime y con el mismo criterio de ADR 0012: solo reescribe lo que
  cambió, con rutas a binarios hermanos.
  `Wants` la inicia con el runtime; `BindsTo`, `After` y `PartOf` hacen que se pare
  con él y participe en sus reinicios. No se habilita un segundo autoinicio.
  Un fallo de bandeja no detiene ni reinicia la autoridad. Al abrir el editor se
  instala/arranca una sola vez de forma idempotente, también si el runtime ya
  respondía, para actualizar unidades anteriores sin reemplazar el cliente sano.
- El clic y «Abrir RAD» activan la instancia del editor, que reclama su nombre
  GApplication antes de abrir la ventana. Si todavía no existe, la bandeja lanza
  el editor hermano en un scope de usuario separado, como las aplicaciones del
  launcher (ADR 0013). La reclamación atómica del nombre resuelve también dos
  arranques simultáneos; una activación durante el arranque queda pendiente hasta
  que exista la ventana. No se mantiene el editor residente.
- «Salir de RAD» cierra primero el editor abierto, activando la acción `quit` de su
  GApplication (sin editor no hay nada que cerrar), y luego pide
  `systemctl --user stop rad-runtime.service`, con plazo y reap del proceso. La
  parada explícita no dispara `Restart=on-failure`; un editor que siguiera abierto no
  rearranca automáticamente por perder la conexión. El autoinicio habilitado no
  cambia: RAD vuelve al abrir el editor o iniciar sesión.

**Límites:** la activación pide al compositor mostrar/desminimizar y enfocar la
ventana, usando también la petición xdg-activation que ofrece winit en Wayland
(`Focus` solo no actúa allí); la política final de foco pertenece al escritorio.
La propiedad del cgroup y la colocación visual de la bandeja se verifican en la
sesión real, no se
certifican con los tests aislados. Estos prueban bus privado, watcher tardío y
reemplazado, activación única, parada sin reinicio solicitada y fin del proceso.
Una compilación parcial sin `rad-tray` no impide arrancar la autoridad.
