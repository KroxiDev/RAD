# 0005 · Tres tipos de ajustes, cada uno con su dueño

**Estado:** aceptada, 2026-10-20.

Hoy el idioma de la interfaz vive en la configuración del dominio, que el runtime
nunca usa, y el uso del dispositivo es un argumento de línea de comandos. Cada
ajuste pertenece a uno de tres tipos:

| Tipo | Ejemplos | Dueño | ¿Viaja en las copias? |
|---|---|---|---|
| Configuración del deck | Carpetas, botones, acciones, imágenes, brillo, reposo, umbral de pulsación, permisos de automatización, ajustes no secretos de los plugins | Dominio (`rad-core`) y Store | Sí |
| Ajustes de la máquina | Usar el Stream Deck con RAD, iniciar con el equipo, plugins instalados y sus credenciales | Runtime local | No; los plugins usados viajan como lista |
| Preferencias del editor | Idioma (por defecto, el del sistema), tema, tamaño de ventana, mostrar «Avanzado» | Editor | No |

Los ajustes de los plugins forman parte de la configuración del deck para que la
regla del usuario quepa en una frase: una copia lleva todo, salvo las contraseñas y
lo propio del equipo. Así además entran en el historial, se pueden deshacer y, si el
plugin falta, se conservan como los parámetros de sus acciones. Se descartó que cada
campo declarase su alcance: complica a autores y editor sin un caso que lo pida.

**Consecuencias:** el formato v2 reserva el lugar de los ajustes de plugins aunque
el contrato que los usa llegue después. Cada ajuste nuevo declara su tipo.
