# 0026 · Aislamiento y evidencia de las pruebas

**Estado** — Aceptada, 2026-11-23.

RAD usa servicios del escritorio, dispositivos y credenciales. El gate debe poder
ejecutarse en paralelo y repetirse sin heredar el estado de la sesión. Los
bancos de rendimiento que sí observan recursos reales necesitan acreditar qué
midieron y qué procesos pueden cerrar. Este ADR recoge los motivos de las
decisiones que ya aplican el [gate](../../scripts/gate.py) y los
[controladores de medición](../../scripts/performance/). El procedimiento y los
límites de evidencia están en [pruebas](../testing.md) y [rendimiento](../performance.md).

## Entorno y caché

La CI usa Arch para ejecutar el mismo gestor de paquetes y las bibliotecas del
producto, con el toolchain fijado. Actualiza el llavero antes del resto de los
paquetes. El contenedor habilita los namespaces que necesita bubblewrap. Las
pruebas de permisos se ejecutan con el UID dueño del checkout y sus temporales,
para que tengan sentido los controles de propiedad del instalador.

Una caché por commit conserva builds recientes aunque coincidan los lockfiles.
Solo una ejecución íntegra puede dejar una estampa de fechas reutilizable. Se
comprueba el contenido y el inventario completo antes de reutilizar fechas de
directorios. En worktrees, el fingerprint de Cargo conserva la ruta absoluta
del registro y el dep-info puede mencionar su ruta relativa al target. De ahí
la caché compartida, el enlace dentro del target secundario y las copias reflink.
El test del propio gate lo ejecuta en un checkout sintético con suites sustitutas
para impedir la recursión.

Los sockets Unix requieren rutas cortas, con margen para los nombres que crea
cada fixture. Las suites que validan el padre de staging reservan sus propios
directorios privados en `/tmp`. Las demás usan XDG y temporales privados por etapa.
Al probar el editor, un bus privado evita activar la ventana instalada, un
sustituto de systemctl impide tocar servicios y la ruta absoluta del socket de
Wayland sigue funcionando tras cambiar el directorio XDG.

## Conformidad y mediciones

El aislamiento de Audio lo aportan el árbol privado, los montajes y el namespace
de red. El plugin descarta variables `PULSE_*`. El fixture reproduce las rutas
del cargador del sistema, incluidas las disposiciones con `/lib64`. Cada acción
del manifest se ejerce con valores conocidos y en orden, como iniciar la
grabación antes de pausarla. Las esperas repiten observaciones y conservan la
ejecución única de cada efecto. En OBS, stderr sólo admite las líneas de eventos
del runtime que forman su journal.

El proceso medido parte de un perfil ya preparado por otro proceso, con el deck
desactivado y los permisos establecidos antes del arranque. Se comprueban todos
los vínculos iniciales y después un representante por fuente compartida para que
el propio muestreo tenga poco coste. La CPU requiere dos muestras, una GPU puede
estar ausente y un reproductor desconectado puede seguir sin datos al vencer la
ventana. Un árbol incompleto invalida el presupuesto. Las comparaciones de builds
rotan las variantes para repartir el efecto de las cachés calientes.

Cada controlador registra los hijos y su identidad antes de entregar una
cancelación pendiente. También espera a terminar el reap si recibe la primera
cancelación durante el cierre. Una señal a un descendiente usa pidfd y comprueba
PID, instante de arranque y sesión. Un `ESRCH` al leer procfs puede significar
que el proceso desapareció después de abrir el descriptor. Un fallo al
identificarlo conserva la incertidumbre. Si hay competencia por el dispositivo,
el controlador detiene su propio escritor sin enviar una imagen de despedida.
La consulta a fuser recibe sólo una ruta absoluta comprobada, porque hay
versiones que interpretan `--` como fin de la lista de archivos.

El controlador de escritorio conserva sólo observaciones de sus ventanas y
procesos. Los bancos con servicios privados usan cookies sintéticas y omiten
salidas que puedan revelar datos personales. En el banco real de OBS el plugin
fijado accede al relay privado, cuya protección de credenciales impide ciertas
lecturas de procfs. Los campos inaccesibles quedan sin medir. Los metadatos
registran alcance y procedencia antes de acceder al servicio.

La medición de wakeups cuenta el TID despertado. En la calibración, un emisor
externo despierta a dos procesos y sólo uno pertenece al conjunto medido. El
acuse por memoria compartida evita añadir despertares por un pipe. La
autenticación de privilegios queda en el agente gráfico y el helper privilegiado
se cierra por EOF o por plazo sin aceptar señales destinadas al lanzador del
usuario. El contador se abre deshabilitado y lee recuentos, sin muestreo en
anillo ni ejecución de copias de intérpretes o binarios setuid.
