# 0013 · Limpieza de procesos: scopes para las aplicaciones y KillMode=mixed

**Estado:** aceptada, 2026-10-20.

Nada de lo que lance un plugin debe sobrevivir al runtime; las aplicaciones lanzadas
desde el deck, sí. Hoy la unidad usa `KillMode=process` para que sobrevivan las
aplicaciones, lo que también impide a systemd matar a los plugins y a sus
descendientes. Los plugins se lanzan sin `PR_SET_PDEATHSIG` ni grupo de procesos, y
`PR_SET_PDEATHSIG` tampoco bastaría: no se hereda en `fork`, así que solo cubre al
hijo directo. Se invierte el mecanismo:

- el helper lanza cada aplicación en su propio scope de systemd, como hacen GNOME y
  KDE, fuera del cgroup del servicio;
- el servicio pasa a `KillMode=mixed`: systemd envía SIGTERM al runtime, que cierra
  en orden, y después mata todo lo que quede en el cgroup, incluidos los
  descendientes daemonizados. Lo mismo ocurre tras una caída, antes de reiniciar;
- cada plugin arranca en su propio grupo de procesos y el host mata el grupo al
  pararlo, lo que limpia los hijos normales mientras el runtime sigue en marcha;
- `PR_SET_PDEATHSIG` queda como refuerzo y para ejecutar el runtime a mano. Se
  configura desde un hilo que viva tanto como el runtime, porque la señal se dispara
  al morir el hilo que lanzó, no el proceso.

**Consecuencias:** un descendiente que salga de su grupo vive hasta que se detiene
el servicio; el estándar lo prohíbe y el arnés lo comprueba con un fixture que crea
un hijo. La limpieza del cgroup depende de systemd y no se puede probar en el sandbox
de tests; la creación del scope se prueba contra un systemd falso en un bus D-Bus
privado.
