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

### Precisión aceptada · 2026-10-20

Una copia del vacío inicial exportada localmente antes de adoptar una copia
extranjera también debe poder restaurarse después en la misma instalación. Solo
para esa copia se admite reasignar la raíz vacía original si su binding `f1` choca
o fue desplazado: contadores de origen `(1,0,0,1)`, única carpeta `f1` con identidad
`(instalación de origen,1)`, sin padre, 32 posiciones vacías y sin botones ni
carpetas hijas. Se restaura su contenido, incluidos ajustes y dependencias no
secretos; si el binding exacto sigue retenido (vigente o retirado), no hace falta
remapearlo. Si ya no está, incluso por poda, se asigna la raíz nueva.

La nueva raíz se asigna normalmente por encima de los rangos reservados. No se
reutiliza la adopción de un destino inicial: el `f1` extranjero permanece retirado
para deshacer. Ninguna entidad de botón cambia. Esta excepción no autoriza
remapear una copia local con contenido o historia, ni debilita la no reutilización
de IDs para scripts.

Una copia lleva la configuración del deck, incluidos los ajustes no secretos de los
plugins ([ADR 0005](0005-tres-tipos-de-ajustes.md)), los medios referenciados, los
contadores de identidad y la lista de plugins usados (id, versión y origen). Quedan
fuera las credenciales, los binarios de plugins, los ajustes de la máquina y las
preferencias del editor. Tras restaurar, el editor ofrece instalar los plugins que
falten y marca los que necesitan contraseña. Fusionar copias queda fuera de alcance.

**Consecuencias:** el formato pasa a RADPKG v2, que sigue leyendo v1
([ADR 0004](0004-evolucion-de-formatos.md)). Restaurar es la operación que valida
una copia y se prueba en los tres destinos.
