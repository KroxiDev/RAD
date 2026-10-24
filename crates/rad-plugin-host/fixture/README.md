# rad-plugin-fixture · fixture independiente de protocolo

Instrumento de inyección de fallos para los tests del host. No depende de crates
de RAD: usa directamente el protocolo público v1 de JSON/u32-BE. Incluye dos
binarios nativos con el ID estable `org.rad.fixture`, versiones 1.0.0 y 2.0.0.
Mantener el mismo ID permite probar actualización y rollback deliberadamente;
las versiones de los binarios difieren.

La acción `run` recibe `mode` obligatorio: echo / fail / slow / crash / late / flood.
Echo informa éxito sin efectos; fail informa unavailable; slow espera 250 ms;
crash sale con código 23; late envía primero una respuesta con la petición anterior
incorrecta y después la correcta; flood envía 1000 respuestas duplicadas. La fuente
`status` devuelve texto v1/v2 para probar TTL.
Con una suscripción activa, slow emite `running` antes de esperar: demuestra la
entrada efectiva en la acción durante los tests de timeout/reap, en vez de limitarse
a vencer el plazo de una petición no enviada.

No usa aplicaciones, sensores, hardware, red ni datos personales, y no escribe en
el sistema de archivos. stdout contiene solo protocolo; todos los frames de stdin
se limitan a 64 KiB. Es deliberadamente un fixture de inyección de fallos, no una
integración de producción ni un sandbox de seguridad.

Compilación independiente desde este directorio, tras la revisión del coordinador,
con caché existente y sin descargas:

```sh
cargo build --offline --release --manifest-path Cargo.toml --bins
```

En RAD, los binarios y tests se habilitan con `rad-plugin-host/fixture-tests`. Las
rutas de fuentes compartidas evitan copiar implementación de RAD al fixture
público. Para el gate del repositorio y los helpers que requiere, seguir
[Pruebas y validación](../../../docs/testing.md).
