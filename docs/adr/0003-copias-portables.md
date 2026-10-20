# 0003 · Copias portables entre instalaciones

**Estado:** aceptada, 2026-10-20.

Una copia debe restaurarse también en otra instalación; hoy falla con
`ShortIdCollision`, porque cada instalación nace con su propio `f1`. Restaurar es una
sola acción que reemplaza toda la configuración y se puede deshacer. El sistema
decide solo qué pasa con los IDs, sin preguntar:

| Destino | IDs |
|---|---|
| Misma instalación | Se conservan |
| Otra instalación nunca editada (sus contadores siguen en el estado inicial) | Se adoptan los del backup; lo creado después usa el token propio de la instalación y contadores por encima de los del backup |
| Otra instalación con contenido | Nuevos, asignados por encima de los contadores del destino **y** de los del backup |

Hay que reservar ambos rangos porque `allocate` parte del máximo del registro local.
Con un destino que llegó a `b1` y un backup con `b1`, `b2` y `b3`, asignar desde el
destino daría `b2`, `b3` y `b4`, y un script del backup dirigido a `b2` actuaría
sobre otro botón. Con la regla, un script antiguo, del destino o del backup, falla
con «no encontrado». «Nunca editada» se define por contadores: un destino editado y
deshecho hasta quedar vacío cuenta como destino con contenido.

Una copia lleva la configuración del deck, incluidos los ajustes no secretos de los
plugins ([ADR 0005](0005-tres-tipos-de-ajustes.md)), los medios referenciados, los
contadores de identidad y la lista de plugins usados (id, versión y origen). Quedan
fuera las credenciales, los binarios de plugins, los ajustes de la máquina y las
preferencias del editor. Tras restaurar, el editor ofrece instalar los plugins que
falten y marca los que necesitan contraseña. Fusionar copias queda fuera de alcance.

**Consecuencias:** el formato pasa a RADPKG v2, que sigue leyendo v1
([ADR 0004](0004-evolucion-de-formatos.md)). Restaurar es la operación que valida
una copia y se prueba en los tres destinos.
