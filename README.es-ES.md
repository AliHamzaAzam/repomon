

<p align="center">
  <img alt="repomon" src="docs/logo.png" width="104">
</p>

# repomon

**Ejecuta una flota de agentes de IA de programación en todos tus repositorios, desde una sola terminal.**

Muchos repos × muchos worktrees × muchos agentes en una sola pantalla. Persistente ante reinicios: los que te están esperando flotan hacia arriba y puedes aprobar un prompt desde tu teléfono.

<p>
  <a href="https://github.com/AliHamzaAzam/repomon/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/AliHamzaAzam/repomon?color=00b3b3&label=release"></a>
  <img alt="License: Apache-2.0" src="https://img.shields.io/badge/license-Apache--2.0-blue">
  <img alt="Platforms: macOS · Linux · Windows" src="https://img.shields.io/badge/macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-555">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-orange">
  <img alt="For Claude Code · Codex · Aider" src="https://img.shields.io/badge/for-Claude%20Code%20%C2%B7%20Codex%20%C2%B7%20Aider-8A2BE2">
</p>

<!-- Hero demo GIF: docs/demo.gif -->
<p align="center">
  <img src="docs/demo.gif" alt="repomon: triaging a fleet of AI coding agents across repos" width="900">
</p>

Otras herramientas ejecutan agentes paralelos en *un repo, muchos worktrees* (Claude Squad, Conductor, Crystal, ccmanager). repomon está diseñado para el desarrollador que maneja **5–15 proyectos activos** con una flota de agentes ejecutándose simultáneamente: **muchos repos × muchos worktrees × muchos agentes**, generados y controlados desde un solo lugar.

```
REPOMON                                              14:02 fri 29 may 2026
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
FLEET   8 agents · 4 repos · 3 need you                    ↑ sorted: needs-you
─────────────────────────────────────────────────────────────────────────

  pos-saas ────────────────────────────────────────────────────────────
  ⏸ wt-checkout  hotfix/checkout-bug     claude  needs you   89↻   3m
  ▶ main         feat/supabase-migration claude  running    142↻  18m
  ○ wt-ui        spike/new-pos-ui                idle              2h

  montage-ai ──────────────────────────────────────────────────────────
  ⏸ wt-mcp       spike/mcp-batch         codex   needs you   44↻   8m
  ▶ main         phase-2-studio-floor    claude  running    201↻   2m

  ↑↓ select   ↵/→ open   spc babysit   n new-lane   / filter   g needs-you   q
```

repomon es una sola herramienta con cuatro **niveles de zoom**, con una selección que te acompaña todo el camino:

- **Flota (Fleet)**: todos los agentes en una sola pantalla; los que te están esperando flotan hacia arriba.
- **Dividido (Split)**: barra lateral de la flota + salida en vivo del agente seleccionado y una línea de entrada.
- **Cuadrícula de supervisión (Babysit)**: mosaicos en vivo ajustados automáticamente a tu ventana; observa y guía a varios a la vez.
- **Enfoque (Focus)**: un agente a pantalla completa con terminal en vivo, entrada y controles completos.

Las teclas de flecha controlan todo (`↵`/`→` hacer zoom, `esc`/`←` alejar, `space` la cuadrícula). Los agentes se ejecutan en sesiones persistentes (tmux en macOS/Linux, procesos host detach en Windows), por lo que sobreviven al cierre de la interfaz y se reactivan (`a`) con el historial completo de desplazamiento. `⏸` marca un agente que te necesita; `g` salta al siguiente.

Más allá de las vistas en vivo, tres paneles de control de Fase 3 (teclas `2`/`3`/`4`): una **línea de tiempo** por repositorio de densidad de commits con correlaciones cruzadas entre repos, **sesiones de trabajo** detectadas (enfocadas vs paralelas, exportables a Markdown) y **búsqueda global** de commits.

Agentes: Claude Code es de primera clase (estado enriquecido desde su transcripción); Codex y Aider también se ejecutan, con una alternativa tmux-alive para cualquier tipo. Consulta [docs/agents.md](docs/agents.md).

**Acceso remoto**: un puente WebSocket opcional protegido con token sirve la misma API JSON-RPC sobre una red privada (Tailscale). El demonio detecta cambios de estado por sesión (incluyendo diálogos de permisos interactivos leídos desde el panel), los emite como `event.notification` y puede enviarlos a dispositivos Apple vía APNs directamente (sin retransmisión). El puente y el protocolo son **abiertos**, por lo que cualquier cliente puede controlarlo hoy mismo (consulta [docs/protocol.md](docs/protocol.md)). Una **app complementaria para iOS** pulida (vista de flota, conversaciones en vivo y un botón Aprobar para diálogos pendientes) está construida y se distribuirá una vez que se disponga de una cuenta de Apple Developer.

## Cómo se compara

|  | **repomon** | Claude Squad / ccmanager | Apps GUI (Conductor, Crystal) | `claude agents` integrado |
|---|---|---|---|---|
| **Alcance** | muchos repos × worktrees × agentes | un repo, muchos worktrees | un repo, muchos worktrees | una herramienta, lista plana |
| **Tiempo de ejecución** | tmux persistente: sobrevive al cierre, permite reconexión | tmux | proceso de app | dentro del CLI |
| **Triage** | los que te necesitan flotan arriba, `g` para saltar | lista plana | varía | agrupado por estado |
| **Límites de uso** | esquina de uso en vivo + autocontinuidad | ✗ | ✗ | ✗ |
| **Remoto** | puente WebSocket abierto + APNs sobre Tailscale (app iOS próximamente) | ✗ | ✗ | ✗ |
| **Vive en la terminal** | ✅ (TUI de 4 zooms) | ✅ | ❌ (GUI) | ✅ |

Opinión honesta: si trabajas en **un solo** repositorio, Claude Squad/ccmanager o una GUI pueden ser más simples. repomon demuestra su valor una vez que ejecutas agentes en **varios** proyectos a la vez.

## Arquitectura

Un demonio en segundo plano (`repomond`) maneja SQLite, observadores de archivos, la capa de git y el entorno de ejecución de agentes, exponiendo una API JSON-RPC a través de un transporte local (socket Unix en macOS/Linux, tubería nominal en Windows). El entorno de ejecución de agentes está detrás de un trait `SessionBackend`: tmux en macOS/Linux, y procesos host por agente en Windows. La TUI (`repomon`) es un cliente ligero. Cinco paquetes:

- `repomon-core`: modelo de datos, capa git gix, almacén SQLite, observadores, entorno de ejecución de agentes (`SessionBackend`).
- `repomon-daemon`: el servidor de sockets/tuberías `repomond` y servicios en segundo plano.
- `repomon-tui`: la interfaz de terminal `repomon`.
- `repomon-mcp`: servidor MCP de repomind (`repomond mcp`), expone la flota a un agente orquestador vía stdio.
- `repomon-host`: `repomon-agent-host.exe`, el host ConPTY por agente que brinda durabilidad estilo tmux en Windows (solo Windows).

## Instalación

**Una línea, sin dependencias** (macOS y Linux, x86_64 / aarch64, incl. WSL2; binarios precompilados, sin Rust ni Xcode):

```sh
curl -fsSL https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.sh | sh
```

**Homebrew** (macOS):

```sh
brew install AliHamzaAzam/tap/repomon      # o: brew tap AliHamzaAzam/tap && brew install repomon
brew services start repomon                # opcional: ejecutar el demonio al iniciar sesión
```

O descarga un tarball del [último lanzamiento](https://github.com/AliHamzaAzam/repomon/releases/latest):
por arquitectura (`aarch64`/`x86_64`) o la compilación `universal`, luego extrae y coloca `repomon` y `repomond` en tu `PATH`.

**Mission Control** (la app de escritorio) se distribuye por separado, para macOS, Windows y Linux, desde el
lanzamiento [`desktop-preview`](https://github.com/AliHamzaAzam/repomon/releases/tag/desktop-preview):

| Plataforma | Archivo |
|---|---|
| macOS (Apple silicon e Intel) | `Repomon_<version>_universal.dmg` |
| Windows | `Repomon_<version>_x64-setup.exe` |
| Linux | `Repomon_<version>_amd64.AppImage`, `.deb` o `.rpm` |

El paquete incluye su propio demonio, por lo que no necesita un `repomond` separado, y se actualiza automáticamente después
de la primera descarga. Controla la misma flota que la TUI y puede operarse enteramente desde el teclado.
Consulta [docs/desktop.md](docs/desktop.md) para la referencia de atajos y configuración.

**Desde el código fuente**: cualquier plataforma con el chain de herramientas Rust (donde no haya un binario precompilado):

```sh
cargo install --git https://github.com/AliHamzaAzam/repomon repomon-tui repomon-daemon
```

En macOS y Linux, repomon necesita `tmux` (los agentes se ejecutan en tmux) y `git` en tiempo de ejecución.
¿No tienes `tmux`? Instálalo: `brew install tmux` (macOS), `sudo apt install tmux` (Debian / Ubuntu / WSL2), `sudo dnf install tmux` (Fedora), `sudo pacman -S tmux` (Arch).

**Windows** (nativo, sin WSL, sin tmux):

El soporte nativo para Windows ya está disponible. Actualmente hay una **compilación de vista previa en la rama
`release/windows-preview`** y **ninguna publicación en GitHub Release aún**, por lo que
el one-liner `irm | iex` a continuación solo funcionará una vez que se etiquete un lanzamiento para Windows.

Hasta entonces, compila desde el código fuente con el chain de herramientas Rust (edición 2024, toolchain 1.95.0) y
una instalación de Git para Windows:

```powershell
git clone https://github.com/AliHamzaAzam/repomon
cd repomon
git switch release/windows-preview
cargo build --release
# repomon.exe, repomond.exe y repomon-agent-host.exe quedarán en target\release\.
# Copia los tres en un directorio en tu PATH (deben vivir juntos).
```

Una vez etiquetado un lanzamiento, el instalador descarga binarios precompilados (sin necesidad del chain de Rust)
y coloca los tres ejecutables en `%LOCALAPPDATA%\Programs\repomon` en tu PATH de usuario:

```powershell
irm https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.ps1 | iex
```

Las variables de entorno reflejan `install.sh`: `REPOMON_INSTALL_DIR` (ubicación de instalación) y
`REPOMON_VERSION` (una etiqueta para fijar en lugar de la última).

Luego, habilita el cambio de directorio al salir agregando a tu perfil de PowerShell (`$PROFILE`):

```powershell
repomon shell-init powershell | Out-String | Invoke-Expression
```

Luego, habilita el cambio de directorio al salir agregando a tu `~/.zshrc` (o `~/.bashrc`):

```sh
eval "$(repomon shell-init zsh)"
```

### Ejecutar el demonio como un servicio (opcional)

La TUI inicia automáticamente `repomond` bajo demanda, por lo que un servicio nunca es obligatorio. Para mantener el demonio
(y sus notificaciones) activo entre inicios de sesión:

```sh
repomon daemon install     # macOS: LaunchAgent de launchd · Linux: unidad de usuario systemd
```

En Linux esto escribe `~/.config/systemd/user/repomon.service`; ejecuta
`loginctl enable-linger` si quieres que `repomond` sobreviva al cierre de sesión.

### Notas de la plataforma Linux

- Las notificaciones de escritorio usan `notify-send` (libnotify); el sonido se reproduce a través de
  `canberra-gtk-play` o `paplay` cuando están presentes.
- La copia al portapapeles usa `wl-copy` (Wayland) o `xclip` (X11); dentro de tmux, la selección por arrastre
  recurre a OSC52 cuando ninguno está instalado. El pegado de imágenes necesita `wl-paste` o `xclip`.
- Las notificaciones con clic para enfocar son exclusivas de macOS (`terminal-notifier`).

### Notas de la plataforma Windows

- **Sin tmux, sin WSL.** En Windows, repomon se ejecuta de forma nativa. Cada agente se ejecuta en su propio proceso host
  detached (`repomon-agent-host.exe`, un ConPTY + emulador de terminal del lado del servidor) que cumple exactamente el rol
  de durabilidad que tmux cumple en Unix: los agentes sobreviven a los reinicios del demonio y se readoptan con historial
  completo de desplazamiento. El demonio se comunica con la TUI y con los hosts a través de tuberías nominales en lugar
  de sockets Unix.
- **Windows Terminal recomendado.** repomon funciona en cualquier consola moderna, pero Windows Terminal
  ofrece el mejor renderizado y es donde el adjuntar emergente abre una nueva pestaña.
- **Requisito mínimo ConPTY: Windows 10 1809.** El host depende de ConPTY, que requiere Windows 10
  versión 1809 o posterior (Windows 11 totalmente compatible). Claude Code en Windows nativo necesita Git
  for Windows.
- **Mantén los tres ejecutables juntos.** `repomon.exe`, `repomond.exe` y `repomon-agent-host.exe`
  deben vivir en el **mismo directorio**; el demonio inicia el host buscando junto a sí mismo.
  `install.ps1` y el zip del lanzamiento ya colocan los tres juntos.
- **Binarios no firmados → SmartScreen.** Los binarios del lanzamiento aún no están firmados digitalmente, por lo que
  Windows SmartScreen puede advertir en la primera ejecución ("Windows protegió tu PC"). Elige *Más información →
  Ejecutar de todos modos*, o desbloquea el zip antes de extraer (`Unblock-File`). La firma está en la
  hoja de ruta.

## Uso

```sh
repomon                                # simplemente ejecútalo: inicia el demonio si es necesario, luego la TUI
repomon add ~/code/pos-saas            # registra un repositorio
repomon discover ~/code --add          # o encuentra y registra muchos a la vez

# sin cabeza / scripting (también inicia automáticamente el demonio)
repomon lane list
repomon lane new --repo pos-saas --branch feat/inventory --source main
repomon lane delete feat/inventory --delete-branch
```

**`repomon` es el único comando.** Sin un demonio en ejecución, lanza un `repomond` detached
(el cual sobrevive entre sesiones de la interfaz), se conecta y abre la TUI. Si el binario de
`repomond` no se puede encontrar, recurre a un demonio en el mismo proceso. Usa `--embedded` para
forzarlo siempre, o administra el demonio con
`repomon daemon start | stop | restart | status | logs | install | uninstall`.

> **¿Construyendo desde el código fuente?** Después de una reconstrucción, ejecuta `repomon daemon restart` para que el nuevo código
> se sirva (el demonio sobrevive a la interfaz). La compilación de desarrollo se ejecuta desde `./target/debug/repomon`.

## repomind — orquestador de flota

repomind es un agente orquestador para la flota: una sesión de `claude` conectada al propio
servidor MCP de repomon, por lo que puede leer el estado de cada carril y actuar en tu nombre — generar trabajadores, responder
a sus solicitudes de permiso y fusionar el trabajo finalizado — mientras tú supervisas o intervienes solo cuando
te lo requiera.

```sh
repomon orchestrate [--autonomy read-only|supervised|autonomous] [--max-agents N] [--model m] [prompt]
```

Esto asegura que el demonio esté activo, inicia (o reutiliza) la única ventana tmux `orchestrator` propiedad del demonio ejecutando `claude`, y te adjunta a ella. `prompt` es un objetivo inicial opcional.

**Centro de comandos de la TUI** (tecla `O`, o `6`): una fila de flota fija más un panel de control para repomind,
accesible como cualquier otro nivel de zoom. La fila y el encabezado escalarán en el momento en que repomind te
necesite — un diálogo de permiso/decisión o una espera al final del turno — y dispararán una notificación de escritorio "repomind te necesita"
cuando la TUI no esté mirando la vista. Presiona `i` para escribir directamente a
repomind sin salir de la vista (`send-keys` mediado); `↵`/`→` se adjunta a su panel tmux real en su lugar.

**Límites de seguridad (Guardrails).** Por decisión de producto, `--autonomy` se predetermina a `autonomous`: repomind puede
crear, fusionar y eliminar carriles y ejecutar un objetivo de extremo a extremo sin preguntar primero, limitado por
unos pocos límites duros aplicados en el servidor (no solo solicitados en el prompt): un límite de acciones por sesión
(100 acciones por defecto), un límite de agentes concurrentes (`--max-agents`, por defecto 4), una deduplicación de 15s
al enviar el mismo texto al mismo carril dos veces seguidas, y un flujo de confirmación humana en dos fases para la eliminación de carriles (la primera llamada solo devuelve un resumen de impacto y un token; la eliminación
solo ocurre una vez que ese token regresa). Pasa `--autonomy supervised` para que proponga la creación de carriles
para que tú los confirmes, o `--autonomy read-only` para limitarlo a la observación.

Antes de fusionar el trabajo de un carril, se espera que repomind lo verifique: `lane_diff` (commits por delante de
la base con diffstat, más cambios sin confirmar) antes de que `merge_lane` los aterrice.

## Integración de shell (cd-al-salir)

Presionar `c` en un carril sale de repomon y cambia tu shell a ese worktree. repomon
escribe la ruta al descriptor de archivo en `$REPOMON_CD_FD`; agrega el wrapper a tu
`~/.zshrc` / `~/.bashrc` para que el shell actúe sobre él:

```sh
eval "$(repomon shell-init zsh)"   # bash: repomon shell-init bash · fish: repomon shell-init fish
```

En **Windows / PowerShell** el wrapper lee la ruta desde un archivo temporal (`$REPOMON_CD_FILE`)
en lugar de un descriptor de archivo heredado; agrégalo a tu `$PROFILE`:

```powershell
repomon shell-init powershell | Out-String | Invoke-Expression
```

## Acceso remoto (puente abierto sobre Tailscale)

El demonio sirve la misma API JSON-RPC a través de un puente WebSocket protegido con token, por lo que puedes controlarlo
desde cualquier cliente; el protocolo está documentado en [docs/protocol.md](docs/protocol.md). Una
app complementaria nativa **para iOS** (vista de flota, conversaciones en vivo, botón Aprobar) está construida y
se distribuirá una vez que se disponga de una cuenta de Apple Developer; hasta entonces, el puente y el emparejamiento `remote pair`
funcionan para cualquier cliente que apuntes hacia ellos. Vincúlalo a la dirección de tu **tailnet privado**,
nunca a una interfaz pública; cualquiera que tenga el token puede leer tus paneles y escribir en tus agentes.

1. **Instala [Tailscale](https://tailscale.com)** en la Mac (y en cualquier dispositivo desde el que te conectarás),
   iniciando sesión en el mismo tailnet, para que pueda alcanzar la Mac en su dirección `100.x.y.z`.
2. **Habilita el puente**, luego reinicia el demonio para aplicar:
   ```sh
   repomon remote enable     # detecta la IPv4 de Tailscale, vincula ws://<ip>:7878, genera un token
   repomon daemon restart
   ```
   ¿No se detecta Tailscale? Pasa la dirección tú mismo: `repomon remote enable --bind <ip:port>`.
3. **Empareja un cliente:** `repomon remote pair` imprime un código QR (y un enlace `repomon://<host:port>#<token>`)
   para que un cliente se conecte.

Gestionarlo con `repomon remote status` (muestra el enlace y un token enmascarado),
`repomon remote enable --rotate-token` (genera un nuevo token, luego vuelve a emparejar) y
`repomon remote disable` (deja de servir; conserva el token). Cada cambio necesita un
`repomon daemon restart` para tener efecto.

## Documentación

- [docs/architecture.md](docs/architecture.md): cómo encajan el demonio, la TUI y el núcleo.
- [docs/desktop.md](docs/desktop.md): Mission Control, sus atajos de teclado y configuración.
- [docs/protocol.md](docs/protocol.md): la referencia del socket JSON-RPC.
- [docs/agents.md](docs/agents.md): cómo se ejecutan los agentes y cómo se detecta el estado.
- [docs/windows-validation.md](docs/windows-validation.md): la puerta de validación manual de extremo a extremo para Windows 11.
- [crates/repomon-host/PROTOCOL.md](crates/repomon-host/PROTOCOL.md): el protocolo de control congelado del host de agente para Windows.

## Estado

La vista de flota (carriles/hoy), el multiplexador de agentes (generación, salida en vivo, entrada, adjuntar,
cuadrícula de supervisión, carriles multiagente), el panel de historial (línea de tiempo/sesiones/búsqueda),
notificaciones por sesión (detección de diálogos de permiso extraídos del panel, disparadas como emergencias de escritorio
incluso cuando la TUI está cerrada o estacionada a pantalla completa en un agente), la capa de acceso remoto
(puente WebSocket + APNs + emparejamiento) y repomind (el orquestador de flota impulsado por MCP —
`repomon orchestrate` y el centro de comandos de la TUI) están todos implementados, en macOS, Linux y Windows.
Cada plataforma tiene rutas nativas para el servicio, notificaciones, portapapeles y vitalidad de proceso/agente. **El soporte nativo para Windows ya está disponible** (código completo y CI verde en
`x86_64-pc-windows-msvc`): un trait `SessionBackend` con un backend tmux en Unix y un
backend de proceso host en Windows, IPC por tuberías nominales y `repomon-agent-host.exe` para
paridad de durabilidad. Aún espera una prueba física de extremo a extremo en Windows 11 y la firma de binarios
antes de que se etiquete un lanzamiento para Windows (consulta [docs/windows-validation.md](docs/windows-validation.md)).
La app complementaria para iOS está construida y se distribuirá una vez que se disponga de una cuenta de Apple Developer.
Diferido: un panel de control web.

---

Si repomon te ahorra unos pocos cambios de contexto al día, un ⭐ ayuda a otras personas a encontrarlo.

## Licencia

Apache-2.0 © Ali Hamza Azam
