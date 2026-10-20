# Completar el contrato

Reemplaza los campos `[PENDIENTE]` de `AGENTS.md` con hechos comprobados de este
proyecto. Aplica `.agentic/golden-rules.md`.

## Procedimiento

1. **Inferir.** Leer el repositorio y proponer un valor para cada campo que tenga
   evidencia: nombre, arquitectura, entrypoints, comandos de validación, framework
   y ubicación de tests, rama por defecto. Fuentes habituales: manifiestos de
   dependencias, scripts de build, configuración de CI, layout de carpetas, `git`.

2. **Confirmar en bloque.** Mostrar los campos inferidos juntos, cada uno con su
   evidencia: `archivo:línea`, o el comando que la produjo. Una sola confirmación
   para todos; el usuario corrige lo que esté mal.

3. **Preguntar lo que falta.** Agrupar en una sola tanda los campos sin evidencia
   —habitualmente propósito, rutas protegidas y dónde vive la documentación
   autoritativa. Preguntar solo lo que cambie el resultado.

4. **Escribir.** Reemplazar en `AGENTS.md` únicamente los campos confirmados.

## Reglas

- Sin evidencia y sin respuesta del usuario, el campo **queda `[PENDIENTE]`**. Un
  valor inventado es peor que un hueco declarado: el hueco se ve, la ficción no.
- El propósito es el *para qué* del proyecto, no una descripción de su código. Si
  no está escrito en ningún lado, preguntarlo; no deducirlo del layout.
- Un comando de validación se propone solo si existe en el proyecto. Ejecutarlo
  para comprobarlo es opcional y necesita autorización; si no se ejecutó, decirlo.
- `[NO APLICA]` es una respuesta válida, y distinta de `[PENDIENTE]`.

## Fin

Termina cuando cada campo tiene un valor confirmado, un `[NO APLICA]`, o sigue en
`[PENDIENTE]` con el motivo dicho en voz alta. Informar cuántos quedaron.
