# 0018 · Distribución Arch y actualización de la instalación

**Estado:** aceptada, 2026-10-25. Implementación y validaciones pendientes se
identifican en el [roadmap](../roadmap.md).

RAD se distribuye como `rust-action-deck` para x86-64, con ejecutables hermanos en
`/usr/lib/rad`. El objetivo inicial es Omarchy 4.0.6; la versión de Omarchy no
congela sus dependencias rolling ni certifica por sí sola un artefacto. La fuente
`packaging/release.json` genera versión del producto, revisión de paquete, tag y
metadata; no versiona contratos de datos o plugins. No se publica desde el build.

## Dueños y contrato

Un subsistema independiente, `rad-update`, es dueño de consultar y actualizar la
**instalación**, nunca de la configuración, el Store, el vault o el Stream Deck.
Su biblioteca expone un contrato tipado acotado al editor y puertas locales de
arranque; sus binarios coordinan el relevo y la transacción. Es una excepción
explícita a la prohibición general de adapters en clientes: no da acceso al dominio
ni compone adapters del runtime. Así no se carga al runtime con una actualización
que debe sobrevivir a su parada, ni se usa el sistema de plugins como instalador.

La UI solo consulta por clic en Ajustes → Actualizaciones. Un worker cancelable y
con generación única evita bloquear egui, trabajos duplicados y respuestas tardías.
Salir de la sección invalida la consulta y el candidato; un error nunca conserva
un resultado anterior. La detección de paquete es local y no requiere runtime sano.
Antes de instalar, el editor resuelve todos sus borradores sin descartarlos por sí
solo. La descarga y verificación finalizan antes del relevo.

El coordinador se lanza como unidad transitoria de **usuario**, fuera del cgroup del
runtime. Adquiere una única exclusión de sesión antes de confirmar el relevo; las
puertas de arranque de editor/runtime/bandeja impiden nuevas instancias durante la
operación. Espera el cierre del editor y pide SIGTERM al proceso principal del
runtime, esperando una parada limpia sin matar el dispositivo por timeout. No
modifica el autoinicio. Si no puede demostrar la parada, no reemplaza binarios.
El estado y diagnóstico son privados, durables y acotados, independientes de los
datos del deck. Un estado incompleto no se convierte en éxito ni se reintenta solo.

## Confianza y privilegios

El usuario reemplazó explícitamente la firma independiente del plan por confianza
en **KroxiDev/RAD por HTTPS**, sin claves adicionales. El control de esa cuenta y
repositorio es la autoridad de publicación: su compromiso permite distribuir código.
Un digest autentica los bytes contra ese origen, no contra una firma independiente.
No se presenta este modelo como equivalente a una clave de firma fuera de GitHub.

La metadata JSON de esquema 1 enlaza producto, paquete, versión estable, `pkgrel`,
arquitectura, archivo, tamaño, SHA-256, base y commit. El tag canónico es
`v<version>-<pkgrel>` y el asset `rad-update.json`. Se enumeran releases paginadas,
no solo Latest; se excluyen draft, prerelease y versiones semánticas prerelease.
La revisión se compara con `vercmp` de Arch, también para `-1 → -2`. Límites de
páginas, bytes, redirecciones y tiempo fallan explícitamente si impiden decidir.
Las URLs y cada redirección se restringen a HTTPS y hosts oficiales previstos.

Solo `rad-update-installer` obtiene privilegios mediante pkexec y autorización por
operación, nunca una contraseña en RAD ni un ejecutor genérico como root. Debido a
la ausencia deliberada de firma independiente, este instalador **vuelve a consultar
por sí mismo el origen HTTPS fijo oficial**: no confía en metadata, digest ni ruta
aportados por el proceso de usuario. Copia los bytes a staging root privado,
rechaza symlinks/sustituciones y verifica identidad, tamaño, digest y metadata del
paquete sobre esa copia antes de invocar pacman. Solo instala el paquete oficial
y una versión superior a la instalada.

Pacman conserva sus controles, bloqueo y política local de firmas. No se baja
`LocalFileSigLevel`, no se usa `--nodeps`, `--overwrite`, `-Sy`, ni se elimina el
lock. Una política que exija firma rechazará un paquete sin firma: el actualizador
explica ese impedimento, no crea una excepción. La transacción se limita al paquete
RAD sin sincronizar o actualizar el sistema; dependencias ausentes/incompatibles
requieren intervención manual. El instalador nunca abre el editor como root.

## Cancelación y recuperación

Consulta y descarga admiten cancelación real y plazos; no dejan workers crecientes.
El usuario puede cancelar antes del relevo. Después de iniciada una transacción,
no se mata pacman por timeout o cierre de UI. Se registra el resultado real,
se consulta la versión instalada y el coordinador de usuario reabre el editor en
Actualizaciones, sin consulta de red automática. Si falla la reapertura, el
diagnóstico persiste y la recuperación manual no borra configuración ni credenciales.
No se promete rollback: una migración de datos no se revierte con un downgrade.

La confianza inicial consiste en descargar del repositorio oficial por HTTPS y
comprobar el nombre del origen. No hay renovación de claves en este modelo;
introducir firmas independientes requerirá una nueva decisión y un bootstrap
explícito, no claves descargadas y aceptadas automáticamente.


## Temporales abandonados

Los temporales del actualizador no son datos ni copias del usuario. La recolección
se limita a sus directorios de staging con UID/tipo/nonce y marcador de dueño,
y a nombres IPC propios que identifican al proceso creador. Sólo un dueño
comprobado como muerto y una lease exclusiva libre permiten recogerlos; un dueño
vivo o desconocido, un archivo sin procedencia, permisos inseguros, enlaces o
contenido inesperado hacen conservar la entrada. No se recorre ni borra
recursivamente contenido ajeno. Los temporales anteriores sin procedencia
suficiente se conservan en vez de inferir abandono por antigüedad.

Las leases compartidas protegen staging durante su uso. El coordinador retiene la
del paquete preparado y pacman hereda las del paquete y de su configuración:
la muerte del instalador no autoriza borrar archivos de una transacción activa.
La identidad viva del creador también conserva un paquete preparado que aún no
se entregó al coordinador. Marcadores y nombres IPC nuevos identifican el namespace
PID del creador; sólo el mismo namespace permite interpretar su PID. Otro namespace
o un formato antiguo se conserva sin sondear procesos. Reutilización de PID o dudas de acceso se resuelven
conservadoramente, sin señales de terminación.

La limpieza es local, acotada y oportunista dentro de trabajos del actualizador;
no introduce red, temporizadores ni mensajes globales. Nunca elimina
`operation.json`, diagnósticos finales, configuración, vault, plugins, backups o
artefactos de compilación. Un fallo de mantenimiento no se confunde con éxito de
una actualización ni autoriza matar procesos o borrar el lock de pacman.
