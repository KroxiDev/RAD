# Distribución e integración de escritorio

Referencia del paquete Arch de RAD y del contrato de sus releases. El
[índice técnico](../docs/README.md), [ADR 0018](../docs/adr/0018-distribucion-y-actualizacion-de-la-instalacion.md)
y [gate canónico](../docs/testing.md) fijan diseño y validación.

## Alcance y estado

Nombre de paquete: **`rust-action-deck`**, arquitectura **x86_64**. Base objetivo:
**Omarchy 4.0.6**, publicada en su [release oficial](https://github.com/omacom/omarchy/releases/tag/v4.0.6).
Omarchy es rolling: ese nombre no fija un snapshot de todas sus bibliotecas.
Cada artefacto declara su ABI glibc mínima real y conserva información del entorno
de construcción. No se presupone compatibilidad con instalaciones antiguas.

El código y la receta no implican que haya una release publicada o certificada.
La publicación y una prueba manual personal son pasos distintos; sus pendientes
vigentes se indican en el [roadmap](../docs/roadmap.md#distribución-y-actualizaciones).
No hay AUR, CI, repositorio pacman propio, AppImage ni canal beta en este flujo.

## Identidad y layout

`release.json` es la única fuente de versión **de producto**, `pkgrel`, nombre,
arquitectura y base. No cambia la versión del documento del deck ni las APIs.
Una revisión de empaquetado también es una actualización: `0.1.0-2 > 0.1.0-1`.
El tag se deriva como `v<version>-<pkgrel>`, sin prerelease ni build metadata.

| Ruta | Contenido |
|---|---|
| `/usr/lib/rad/` | `rad-editor`, `rad-runtime`, `rad-tray`, `rad-launcher-helper`, `rad-cli`, `rad-update-coordinator`, `rad-update-installer` |
| `/usr/bin/rad-editor`, `/usr/bin/rad-cli` | Enlaces a sus ejecutables hermanos |
| `/usr/share/applications/io.github.KroxiDev.RAD.desktop` | Menú con ruta estable al editor |
| `/usr/share/pixmaps/io.github.KroxiDev.RAD.png` | Ícono de origen, sin duplicar identidad |
| `/usr/lib/systemd/user/` | Plantillas de runtime y bandeja con rutas estables |
| `/usr/share/polkit-1/actions/` | Autorización de **solo** el instalador limitado; `auth_admin`, sin autorización persistente |
| `/usr/share/licenses/rust-action-deck/` | Licencia RAD y textos de dependencias incluidas |
| `/usr/share/rad/release.json` | Identidad del producto instalado |

`rad.png` sigue siendo la única fuente gráfica de ventana, menú y bandeja. Es la
K provisional del GitHub del autor; se sustituye reemplazando ese PNG y
recompilando. El app_id estable es `io.github.KroxiDev.RAD`.

El editor conserva la gestión de las unidades de usuario de ADR 0012/0016. Al
abrir explícitamente la instalación del paquete reconcilia sus unidades y la entrada de menú con
los ejecutables hermanos en uso; así reemplaza referencias anteriores a
`target/release`. El mismo app_id evita dos entradas de menú. Los scriptlets root
no inspeccionan hogares, no tocan datos del deck ni activan servicios de usuarios.
La configuración, medios, plugins, credenciales y preferencias siguen en XDG,
fuera del paquete. No se distribuyen plugins oficiales dentro de RAD.

## Construir sin instalar ni publicar

Prerequisitos: caché Cargo y lockfiles completos, toolchain compatible, herramientas
de construcción Arch (`makepkg`, `fakeroot`, `bsdtar`, `tar`, `zstd`, `readelf`),
Python 3 y dependencias de compilación del workspace. No se descargan o instalan
herramientas por este procedimiento. La base debe ser Omarchy 4.0.6 x86_64.
Usar un entorno de construcción limpio para una release; revisar también los
metadatos `.BUILDINFO` de makepkg antes de distribuirlos.

Desde la raíz, como usuario normal:

```bash
python3 packaging/release.py build --plan  # Precondiciones e intención, sin compilar
python3 packaging/release.py build         # Requiere fuente limpia; NO instala
```

La herramienta fija el target `x86_64-unknown-linux-gnu`, usa Cargo offline/locked,
features de producción y binarios explícitos; excluye ejemplos, fixtures, cachés y datos personales. Compila en
`target/`, serializa Cargo y remapea rutas de fuente. `PKGBUILD.in` se materializa
con versiones y checksums reales; `makepkg` trabaja sobre un bundle local, sin
`--syncdeps`. No toca ramas, tags, remotos o releases.

Los resultados quedan en una carpeta nueva bajo `target/dist/`: paquete
`.pkg.tar.zst`, `rad-update.json`, `SHA256SUMS`, receta, bundle y entorno de
construcción. No sobrescribe una entrega anterior. El mínimo glibc se obtiene de
los símbolos ELF de **ese** build, no de una suposición sobre Omarchy.
Los [notices recuperados](notices/README.md) tienen procedencia y hashes fijados
por revisión. El build los usa solo para la versión y commit correspondientes;
conserva también los avisos individuales de fuentes. Si falta otro texto, se
detiene para revisión: no inventa notices ni descarga licencias por su cuenta.
Esta preservación no certifica por sí sola un inventario legal exhaustivo.
La receta fuerza `--nosign` para no usar claves privadas de la máquina y fija
extensión/rutas/packager en una configuración local. `makepkg` se ejecuta desde
staging neutral, no desde el checkout; la metadata rechaza `startdir`/`builddir`
personales en `.BUILDINFO`. Esto no cambia la política de firmas de pacman.

Para probar cambios sin commit existe deliberadamente otra modalidad:

```bash
python3 packaging/release.py build --development
# Solo para iteración local, sin evidencia de compilación de producción:
python3 packaging/release.py build --development --reuse-build
```

Esta modalidad permite un Working Tree sucio y marca `DEVELOPMENT-NOT-FOR-RELEASE`.
**No genera metadata de actualización publicable.** No subir ese artefacto como
release estable ni confundir `--reuse-build` con un build reproducible.

## Preparar la primera publicación

1. Probar el paquete local con la sesión de RAD cerrada, conservando los datos y
   una copia fuera de sus carpetas. No hace falta desinstalar un build del checkout
   ni borrar el repositorio: cerrar el editor **y** el runtime basta para el relevo.
2. Instalar el paquete con pacman y abrir `/usr/bin/rad-editor` una vez como usuario
   normal. Comprobar menú, configuración, medios, plugins, autoinicio y copias. Las
   credenciales no están en la copia portable: conservar los datos locales.
3. Tras aprobación del trabajo, registrar y publicar la revisión de código según
   el flujo beta → main del proyecto. Esto requiere autorización: la preparación
   local no implica commit, push, promoción, tag o publicación.
4. Elegir versión/revisión en `release.json`, construir desde fuente limpia con el
   procedimiento de producción y repetir la prueba sobre **ese artefacto**. Los
   paquetes `DEVELOPMENT-NOT-FOR-RELEASE` y `--reuse-build` no son candidatos públicos.
5. Verificar identidad, dependencias/ABI, notices, ausencia de datos/rutas privadas,
   digest, metadata y nombre disponible. Preparar una release con el tag canónico,
   adjuntar el paquete, `rad-update.json` y checksum, revisar y sólo entonces
   publicar como estable si realmente está lista. No marcar una beta como estable
   para hacerla visible al actualizador. [Procedimiento de GitHub](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository).

La instalación inicial y el reemplazo de paquete usan `pacman -U`; no requieren
borrar previamente los datos o retirar el paquete instalado.
[Operación documentada por pacman](https://man.archlinux.org/man/pacman.8.en#OPERATIONS).
La retirada/reinstalación es un caso de prueba separado, no un requisito para
actualizar: cerrar RAD antes de cada reemplazo y conservar XDG y copias.

Para verificar el flujo integrado de Actualizar RAD hacen falta una instalación
gestionada y una versión/revisión estable **superior** publicada con su metadata
oficial. Una lista vacía sólo permite comprobar «No hay actualizaciones»; un draft
o prerelease no habilita el botón. La primera release no prueba por sí sola una
actualización entre dos versiones. Antes de afirmar ese recorrido, comprobar
confirmación/cancelación, autorización del sistema, parada, pacman, versión real,
reapertura y conservación de datos con una publicación posterior válida, sin
bypass de origen ni release ficticia.

## Contrato de una release oficial

Publicar requiere un encargo separado y un artefacto verificado. La release debe
ser estable de verdad, no una beta remarcada para hacerla visible al actualizador.
Subir exactamente el paquete y su `rad-update.json`; el tag es el derivado de la
misma fuente. `SHA256SUMS` es una ayuda de inspección, no una firma independiente.
No adjuntar paquetes ambiguos con el mismo nombre.

La metadata de esquema 1 tiene exactamente estos campos:

- `schema`, `product`, `package`, `version`, `pkgrel`, `architecture`, `base` de
  `release.json`;
- `file`: `rust-action-deck-<version>-<pkgrel>-x86_64.pkg.tar.zst`;
- `size`: cantidad exacta de bytes, positiva y como máximo 512 MiB;
- `sha256`: digest hexadecimal minúsculo de esos bytes;
- `commit`: revisión completa de fuente, 40 dígitos hexadecimales minúsculos.

La herramienta valida también `.PKGINFO`; renombrar otro paquete no basta. El
cliente consulta el origen fijo `KroxiDev/RAD`, enumera páginas y compara con
`vercmp`, no por fecha, orden alfabético o Latest. Un límite que impide decidir,
una respuesta malformada o un asset ausente/ambiguo producen error, no «al día».
Los límites concretos de red y representación están en
[`rad-update`](../crates/rad-update/src/lib.rs).

### Confianza inicial

Por decisión explícita del usuario, se confía en **GitHub oficial por HTTPS**, sin
claves adicionales ni firma independiente del paquete. Confirmar que se está en
`https://github.com/KroxiDev/RAD/releases` y que la release no es prerelease. La
seguridad depende de la cuenta/repositorio, GitHub, TLS y las autoridades de
certificación del sistema. Quien controle ese origen puede publicar actualizaciones
con privilegios de instalación. Un checksum del mismo origen no protege contra su
compromiso; no se afirma lo contrario.

El instalador elevado no cree el checksum que le entrega el usuario: vuelve a
obtener la metadata desde el origen fijo oficial y verifica una copia root privada
del paquete. No acepta una ruta como permiso para instalar cualquier paquete.
Pacman conserva la política de firmas local. Si `LocalFileSigLevel` exige una
firma, un paquete sin firma será rechazado; **no bajar esa política** para sortear
el error. Incorporar firmas independientes requiere un diseño y bootstrap nuevos.

## Instalar y actualizar manualmente

Estos comandos modifican la instalación y requieren autorización administrativa;
la existencia del ejemplo no autoriza ejecutarlos en una máquina personal.
Antes de reemplazar o retirar RAD, guardar borradores y elegir **Salir de RAD**
en la bandeja. Cerrar solo la ventana no detiene el runtime.

Tras descargar el paquete desde la release oficial y comprobar su metadata:

```bash
sudo pacman -U ./rust-action-deck-<version>-<pkgrel>-x86_64.pkg.tar.zst
```

En una instalación nueva, abrir RAD desde el menú. Si antes se ejecutó un
checkout, su entrada de menú local puede sombrear la del sistema: cerrar primero
RAD y ejecutar **`/usr/bin/rad-editor`** una vez como usuario normal. Ese arranque
reconcilia las rutas locales con el paquete; después usar el menú. No ejecutar el
editor como root. No usar `pacman -Sy`, no borrar `db.lck` ni omitir dependencias o firmas.
Si las dependencias requieren actualizar Omarchy, resolverlo como mantenimiento
del sistema separado, no como parte de Actualizar RAD.

En la instalación gestionada, **Ajustes → Actualizaciones → Verificar
actualizaciones** es la única consulta de red. No se consulta al abrir RAD ni al
entrar en la sección. Con un candidato superior aparece **Actualizar RAD**;
confirmar resuelve borradores, descarga y verifica antes de parar. Un checkout
puede consultar, pero no sustituye la instalación del sistema.
La instalación solicita autorización del sistema; RAD no recoge contraseñas.
Si hay exclusiones `NoUpgrade` o `NoExtract` configuradas, el actualizador las
rechaza conservadoramente antes de parar RAD; no las modifica ni declara éxito
solo por la versión de la base de pacman. Usar mantenimiento manual revisado.
El coordinador mantiene el autoinicio, instala solo RAD y reabre la sección bajo
el usuario original. Una transacción iniciada no se mata para cancelar o por el
plazo de una consulta. No hay downgrade ni reintento automático.

## Limpieza automática de temporales

Los trabajos del actualizador recogen oportunistamente su staging abandonado y
los archivos IPC transitorios; no hay timer, consultas de red o avisos globales
para limpiar. Las comprobaciones y lecturas de resultado se ejecutan en workers;
el instalador autorizado mantiene aparte los temporales root.

Sólo se recoge un temporal reconocido, privado y del UID esperado cuyo creador
está comprobado como muerto en el mismo namespace PID y cuya lease está libre.
Un paquete preparado o un proceso vivo siguen protegidos; el coordinador conserva
su lease y pacman hereda las del paquete y configuración. Un cierre del lanzador
no permite retirar archivos que la transacción todavía usa.

Se conservan enlaces, contenido extraño, permisos inseguros, dueños desconocidos,
formatos anteriores sin procedencia suficiente y namespaces distintos. El recorrido
es acotado y no borra árboles recursivamente. No se elimina el resultado de una
operación, los diagnósticos finales, las copias, datos XDG, credenciales, plugins,
artefactos de compilación o el lock de pacman. Nunca usar un borrado general de
`/tmp/rad*` como sustituto de esta política. Reglas y límites de propiedad:
[ADR 0018](../docs/adr/0018-distribucion-y-actualizacion-de-la-instalacion.md#temporales-abandonados).

## Retirar, reinstalar y recuperar

Antes de desinstalar, guardar cambios y elegir **Salir de RAD**; comprobar que no
quedan runtime/bandeja activos en **esa sesión**. No detener controladores ajenos.
Después, con autorización, `sudo pacman -R rust-action-deck`. No usar `-Rns` por
inferencia. Pacman no elimina datos XDG, credenciales ni preferencias de autoinicio;
las unidades personales quedan para reinstalación, sin procesos activos si se
siguió la parada previa. No abrirlas mientras falten los ejecutables. Reinstalar
el paquete y abrir RAD recupera los datos existentes sin copiarlos del desarrollador.

Si una operación se interrumpe, consultar **Ver detalles** en Actualizaciones y el
estado privado en `$XDG_STATE_HOME/rad/update` (por defecto
`~/.local/state/rad/update`). No editar el Store para reparar el instalador. Una
fase incompleta no acredita éxito y no dispara otra instalación al arrancar.
Verificar la versión real con `pacman -Q rust-action-deck`. Si no abre el editor,
conservar el diagnóstico y el registro de pacman y reinstalar manualmente el
**mismo paquete compatible** desde el origen oficial, tras parar la sesión.

No se promete rollback: volver a un binario antiguo puede no entender datos ya
migrados. No borrar configuración ni hacer downgrade a ciegas. Las copias RAD
exportadas no contienen credenciales y no sustituyen un respaldo completo. Para
una prueba manual personal, realizar antes un respaldo privado y coherente de los
datos XDG y preferencias, con RAD detenido; no añadirlo al repositorio ni publicar
su contenido o evidencia personal.
