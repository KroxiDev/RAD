# RAD · configuración y control del Stream Deck

RAD organiza las teclas de un Stream Deck en carpetas y botones con acciones y
apariencias. Este glosario fija el lenguaje del dominio para desarrolladores y
agentes. `_Evitar_` lista sinónimos descartados; `_No confundir con_`, conceptos
distintos que suelen mezclarse.

## Lenguaje

### Superficie y organización

**Stream Deck**:
Periférico de teclas que RAD usa como superficie de control; su modelo determina
la rejilla. En compuestos y textos breves se abrevia «deck».
_Evitar_: dispositivo, controlador.

**Tecla**:
Control físico del Stream Deck que ocupa un lugar en su rejilla.
_Evitar_: botón físico.
_No confundir con_: botón, posición.

**Rejilla**:
Disposición de las teclas en filas y columnas, compartida por todas las carpetas
de la configuración del deck.
_Evitar_: distribución, layout.

**Posición**:
Lugar de la rejilla dentro de una carpeta, vacío u ocupado por un botón.
_No confundir con_: tecla, identidad del botón.

**Botón**:
Elemento configurado que ocupa una posición y reúne nombre, apariencia, permisos
y plan. En los textos de la interfaz se presenta como «tecla».
_No confundir con_: tecla, acción.

**Apariencia**:
Presentación de un botón mediante texto e imagen, complementada cuando corresponde
por un visual dinámico.
_No confundir con_: nombre del botón.

**Carpeta**:
Organización de posiciones dentro del árbol de navegación del deck. Su nombre de
presentación, cuando tiene entrada, es el del botón de entrada.
_Evitar_: página, perfil.

**Carpeta principal**:
Raíz del árbol de carpetas, sin carpeta superior.
_Evitar_: perfil principal.

**Carpeta activa**:
Carpeta en la que se encuentra la navegación del deck; contra ella se interpretan
las teclas.
_Evitar_: carpeta actual.

**Botón de entrada**:
Botón de navegación situado en una carpeta que representa el acceso a una de sus
carpetas hijas.

**Botón de retorno**:
Botón de navegación de una carpeta hija que representa la vuelta a su carpeta
superior.

### Acciones y ejecución

**Acción**:
Paso configurado de un botón, con identidad propia: un tipo de acción con sus
parámetros.
_No confundir con_: tipo de acción, ejecución.

**Tipo de acción**:
Operación que se puede configurar como acción, nativa de RAD, como abrir una
aplicación o Esperar, o declarada por un plugin.
_No confundir con_: acción.

**Plan**:
Conjunto ordenado de acciones de un botón, con un modo de ejecución secuencial o
paralelo y un gesto común.
_Evitar_: macro, secuencia.

**Gesto**:
Forma de interacción física que activa el plan de un botón: pulsar, mantener o
pulsación larga.
_Evitar_: disparador.
_No confundir con_: acción, ejecución pedida por el editor o la automatización.

**Admisión**:
Aceptación de una ejecución por el runtime, que valida el plan o la acción y fija
la configuración con la que se ejecutará.
_No confundir con_: pulsación, ejecución terminada.

**Ejecución**:
Ocurrencia admitida de un plan o de una sola de sus acciones, con resultado propio.
_No confundir con_: acción, pulsación.

**Reposo del deck**:
Estado del Stream Deck con las pantallas apagadas que conserva la configuración y
la carpeta activa. La acción nativa que lo activa se llama «Suspender Stream Deck»
en la interfaz.
_No confundir con_: suspensión del equipo, cierre de RAD.

### Configuración y pertenencia

**Configuración del deck**:
Contenido portable de RAD: carpetas, botones, acciones, apariencias, brillo,
reposo, umbral de pulsación, permisos y ajustes de los plugins.
_Evitar_: perfil.
_No confundir con_: ajustes de la máquina, preferencias del editor.

**Ajustes de la máquina**:
Opciones y recursos propios de la instalación local: usar el Stream Deck con RAD,
iniciar con la sesión, los plugins instalados y sus credenciales.
_No confundir con_: configuración del deck.

**Preferencias del editor**:
Elecciones locales sobre la presentación del editor, como el idioma.
_No confundir con_: configuración del deck, ajustes de un plugin.

**Ajustes de un plugin**:
Valores no secretos que comparten todos los usos de un plugin en la configuración
del deck.
_No confundir con_: parámetros de una acción o de un visual, credencial.

**Credencial**:
Dato secreto local que permite a un plugin acceder a una integración. En la
interfaz se presenta como «contraseña».
_No confundir con_: parámetro de una acción, ajuste de un plugin.

**Copia**:
Conjunto portable de la configuración del deck y de los recursos necesarios para
restaurarla, sin secretos ni recursos propios de la máquina.
_Evitar_: backup, respaldo, exportación.
_No confundir con_: paquete de plugin.

### Identidad y permisos

**Entidad**:
Carpeta, botón o acción con identidad propia dentro de la configuración.

**Identidad**:
Distinción permanente de una entidad respecto de cualquier otra, independiente de
su nombre y posición.
_No confundir con_: nombre, posición, identificador corto.

**Identificador corto**:
Referencia abreviada de una entidad, como `b12`, que usa la automatización;
distinta de su nombre de presentación.
_No confundir con_: identidad, posición.

**Permisos de automatización**:
Autorizaciones para consultar, modificar o ejecutar contenido del deck desde la
automatización.
_No confundir con_: confianza en un plugin, permisos de acceso al Stream Deck.

**Origen de petición**:
Procedencia de una petición de cliente, distinguida entre editor y automatización.
_No confundir con_: origen de plugin, gesto.

### Plugins y visuales

**Plugin**:
Extensión de RAD que aporta tipos de acción o tipos de visual para una
integración.
_No confundir con_: paquete de plugin, acción.

**Paquete de plugin**:
Unidad distribuible con el ejecutable y la descripción de un plugin.
_No confundir con_: copia.

**Origen de plugin**:
Procedencia registrada de la distribución de un plugin; no acredita a su autor.
_No confundir con_: origen de petición.

**Confianza en un plugin**:
Decisión del usuario, al instalar un plugin, de ejecutar su código nativo sin
aislamiento ni firma.
_No confundir con_: permisos de automatización.

**Selección de plugin**:
Paquete instalado que el runtime usa para un plugin; cambia al instalar,
actualizar, reinstalar o volver atrás, no al reiniciar su proceso.
_No confundir con_: opción dinámica, elemento seleccionado en el editor.

**Tipo de visual**:
Clase de visual dinámico que declara un plugin para que un botón la use en su
apariencia.
_Evitar_: fuente de visual.

**Visual dinámico**:
Representación de una fuente observada en la apariencia de un botón, independiente
de las acciones de su plan.
_No confundir con_: acción, imagen estática.

**Fuente observada**:
Estado externo que un plugin observa para producir un visual dinámico, como la
escena activa de OBS.
_No confundir con_: tipo de visual, fuente de OBS.

**Vigencia visual**:
Condición de un visual dinámico mientras no vence el plazo que declaró el plugin
que observa su fuente.
_Evitar_: frescura, validez, TTL.
_No confundir con_: proceso del plugin vivo.

**Opción dinámica**:
Valor que un plugin ofrece en el momento para un parámetro o ajuste, como una
escena de OBS; la configuración guarda la opción elegida.
_Evitar_: selección.

### Estado y comunicación

**Revisión**:
Versión guardada y numerada de la configuración del deck; cada cambio aplicado
publica una nueva.
_No confundir con_: revisión de una vista o de un aviso, que solo indica cuándo
releer.

**Aviso**:
Señal del runtime a un cliente suscrito de que una vista cambió y debe releerse;
no lleva su contenido.
_No confundir con_: mensaje al usuario, revisión.

**Modo recuperación**:
Estado del runtime cuando no puede abrir la configuración: sigue atendiendo al
editor, pero no usa el Stream Deck ni lanza plugins.
_No confundir con_: almacenamiento incierto, vuelta a la revisión anterior.
