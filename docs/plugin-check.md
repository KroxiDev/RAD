# Arnes de compatibilidad de plugins · v1

`rad-plugin-check paquete.radp escenario.json` ejecuta código **nativo de confianza**,
no ofrece sandbox ni verifica autoría. No usar con servicios personales para probar
compatibilidad: preparar servicios privados y binarios fixture. El paquete conserva
el manifest real del plugin y lleva el ejecutable fixture correspondiente.

La salida stdout es un objeto JSON con `version: 1`, `status` (`PASS` o
`NO_VERIFICADO`), `checks` (comprobaciones completas) y `failure` (código estable o
null). Cualquier comprobación fallida, incompleta o no observable termina con salida
no cero. stderr contiene la advertencia de confianza; no imprimir credenciales.

## Escenario

```json
{
  "version": 1,
  "trusted_native_code": true,
  "timeout_ms": 2000,
  "freshness_ms": 26000,
  "args": [],
  "actions": [
    {"id": "example", "gesture": "press", "parameters": {"schema_version": 1, "values": {}}}
  ],
  "options": [],
  "visuals": [
    {"id": "state", "parameters": {"schema_version": 1, "values": {}}}
  ],
  "pressure": {"kind": "visual", "index": 0, "safe_to_repeat": true}
}
```

- Cubrir **todas** las acciones y cada gesto declarado: `press`, `hold` (inicio y
  fin con reserva Host) y `long_press`. Omitir `gesture` equivale a `press`.
- Cubrir todos los visuales, y cada campo dinámico mediante una entrada `options`
  con `scope` (`action`/`visual`), `definition`, `field` y `context` (Parameters).
- Si hay esquema de ajustes, incluir `settings` (Parameters). No se inventan
  parámetros ni valores de ajustes. Un campo dinámico en `settings` no es
  certificable: el contrato v1 todavía no tiene scope de opciones para ajustes.
- `args` se pasa como argumentos literales, sin shell. Para servicios privados,
  arrancarlos desde el orquestador y pasar sólo las rutas/endpoints fixture.
- `timeout_ms`: 100–10000; cada respuesta debe tener éxito y ser observable.
  Opciones `unavailable` no demuestran que el vector ejerció la consulta.
- `freshness_ms`: 1100–65000; debe superar el plazo de vigencia del **primer** estado observado.
  El visual debe mantenerse Current y emitir al menos una renovación sin carga.
  Otro proceso comprueba que, tras la baja y el plazo de respuesta, no llegue
  ningún estado ni renovación durante ese mismo intervalo.
- `pressure`: índice cero basado de `actions`, `options` o `visuals`, con `kind`
  singular `action`, `options` o `visual`. Repite **únicamente** ese vector y exige
  consentimiento `safe_to_repeat: true`; jamás seleccionar una acción con efectos
  reales para generar presión. Una fuente visual alterna alta/baja. Antes de dejar
  de leer, observa una respuesta del vector: el frame que el plugin no pudo escribir
  puede ser esa respuesta, mayor que los ya encolados (por ejemplo, rechazos `busy`).
  La prueba observa el cierre dentro de `timeout_ms`; no certifica un presupuesto universal
  de 50 ms ni el código de salida 74. Esa garantía de `run_stdio` del SDK se valida
  por separado con sus tests.
- El checker inyecta una credencial sintética conocida por cada slot declarado;
  fixtures autenticados deben aceptar ese valor. No introducir secretos reales en
  el JSON, argumentos o variables del orquestador.

Los límites de escenario son 128 acciones, 256 consultas y 32 visuales simultáneos.
El Host y los probes raw tienen presupuestos temporales, de entrada y salida; el
checker mata y recoge su grupo propio al fallar. Esa limpieza no convierte un EOF
con hijos vivos en una comprobación exitosa.

## Integración de los cuatro oficiales

Los scripts de conformidad deben conservar sus controles semánticos específicos y
sumar este arnés, no sustituirlos por `PASS`. El patrón común es:

1. Construir fixture y helpers offline con la caché del repositorio.
2. Arrancar `pactl` falso (audio), bus D-Bus privado con reproductores falsos
   (media), servidor OBS loopback efímero o archivos/NVML falsos (metrics).
3. Empaquetar el ejecutable fixture con `plugins/<nombre>/manifest.json` usando
   `rad_plugin_package::{Candidate::from_parts, encode}`.
4. Generar escenario completo para ese manifest y servicios privados. Elegir una
   consulta de opciones repetible o una fuente fixture para presión.
5. Ejecutar el mismo `target/release/rad-plugin-check` y guardar su JSON en
   `target/conformance/<nombre>/`; salida no cero falla la conformidad.
6. Apagar y recoger servicios privados incluso si falla el checker.

Los orquestadores Python no deben construir peticiones del socket runtime a mano:
usar la salida JSON versionada de `rad-cli` o un cliente Rust tipado en su frontera.
El protocolo raw de plugin dentro del checker usa exclusivamente
`rad-plugin-contract::{Envelope,Message,encode,decode}`.

La implementación oficial se ejecuta desde los tres `scripts/conformance/*/run.sh`.
`audio-media/checker.py` comparte la preparación aislada de los cuatro escenarios;
la cobertura se obtiene del manifest real, pero los parámetros y la fuente repetible
son explícitos. Conserva `<plugin>-scenario.json`, `<plugin>.radp`,
`<plugin>-checker.json` y stderr junto a las regresiones semánticas de cada run.
OBS usa sólo un puerto efímero (también como ajuste `port`) y una credencial canario;
media recibe una dirección de bus privado explícita y audio una ruta de `pactl` falso.
Métricas usa el lector determinista, sin consultar procfs/NVML de la máquina.

Los vectores de los `check.py` pasan por el example Rust `conformance_client`:
`Request::parse` de `rad-api` → `rad-client::Client::call` → salida CLI JSON v1.
No contiene una copia del esquema ni framing/parsing del socket en Python. El mismo
helper empaqueta mediante `Candidate::from_parts` y `encode` del host.
