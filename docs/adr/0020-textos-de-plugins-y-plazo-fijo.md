# 0020 · Textos de los plugins en el idioma del usuario y plazo fijo de respuesta

**Estado:** aceptada, 2026-10-30.

El usuario pide que los plugins, y OBS en particular, sean más simples de usar:
cada acción debe ser útil y entenderse sin conocer su mecanismo. El editor
mostraba los textos de los plugins en inglés y las opciones fijas por su valor
interno (`toggle`, `on`, `off`, `show`), sin ninguna elegida. El manifest solo
tenía etiquetas en inglés y opciones sin texto propio, y el catálogo del editor
no puede traducir a un plugin sin ramas específicas para él
([arquitectura](../architecture.md#dónde-vive-cada-cambio)). Además, OBS
necesita esperar a que la aplicación termine de abrir más allá de los cinco
segundos que el host da a una acción sin un campo de plazo visible para el
usuario.

## Textos de los plugins

La capacidad aditiva `labels` del contrato v1 permite:

- **Opciones fijas con texto propio:** cada opción de un campo `choice` puede ser
  `{"value":"toggle","label":"Toggle"}` además de la cadena simple anterior, que
  sigue leyéndose como opción cuyo texto es su valor. Se guarda siempre el valor;
  la API local sigue listando los valores de las opciones fijas.
- **Traducciones en el manifest:** `translations` asocia cada idioma (2–3 letras
  minúsculas, hasta 16) con hasta 512 textos del manifest y su traducción. La
  clave es el texto tal como lo escribe el manifest.

El editor muestra cada texto de un plugin (acción, visual, campo, opción y los
textos fijos de sus listas dinámicas, como el dispositivo predeterminado) en el
idioma del usuario y, sin traducción, el texto del plugin. Es la excepción
explícita a que todo texto visible salga del catálogo del editor: un plugin es
dueño de sus textos y los trae en su manifest. El runtime sigue sin redactar
prosa.

Una opción obligatoria de un borrador nuevo empieza en la primera opción del
plugin (S4 de la [norma de UX](../ux.md)); los plugins oficiales ponen primero
la recomendada, como «Alternar».

## Plazo fijo de respuesta

`response_timeout` admite omitir `field`: la acción tiene siempre
`default_seconds`, entre 1 y 120, dentro de la misma capacidad
`response_timeout`. Un campo nombrado conserva sus reglas anteriores.

## Consecuencias

- Un host anterior rechaza, por su parseo estricto, un manifest que usa `labels`
  o un plazo sin campo; los plugins oficiales 0.3.0 requieren un RAD con este
  contrato. La especificación para autores en `KroxiDev/rad-plugins` debe
  describirlo en la misma entrega que los publique.
- OBS es el primer uso: sus acciones declaran 20 segundos, esperan hasta 15 a
  que OBS acepte la conexión y se ejecutan de a una. Una petición que ya llegó a
  OBS nunca se repite al reconectar (E5); los detalles están en el
  [README del plugin](../../plugins/obs/README.md).
- Pruebas de referencia: [labels.rs](../../crates/rad-plugin-contract/tests/labels.rs),
  [official_texts.rs](../../crates/rad-runtime/tests/official_texts.rs) y las
  pruebas de borradores en [plugins.rs](../../crates/rad-editor/tests/plugins.rs).
