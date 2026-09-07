# demo.tape — VHS script for repomon's hero GIF.
#
#   Install:   brew install vhs        (pulls ttyd; ffmpeg already required)
#   Generate:  vhs demo.tape           →  writes docs/demo.gif   (then commit the gif)
#              https://github.com/charmbracelet/vhs
#
# A hidden alias points `repomon` at the ISOLATED demo instance (own socket/DB/tmux +
# throwaway sample repos) so the real fleet/projects never appear, while the GIF shows
# the plain `repomon` command. The tour only NAVIGATES (never types into an agent).

Output docs/demo.gif
Require repomon

Set Shell "zsh"
Set FontSize 16
Set Width 1700
Set Height 950
Set Padding 16
Set Theme "Catppuccin Mocha"
Set TypingSpeed 90ms

# --- hidden setup: make `repomon` launch the isolated demo, then clear the screen ---
Hide
Type "alias repomon='env XDG_CONFIG_HOME=/tmp/rmdemo/cfg REPOMON_DATA_DIR=/tmp/rmdemo/data ~/.cargo/bin/repomon --socket /tmp/repomon-demo.sock'"
Enter
Type "clear"
Enter
Show

# --- launch: it's one command ---
Type "repomon"
Sleep 600ms
Enter
Sleep 4s              # let the fleet render

# --- 1. the fleet: agents across repos, the ones waiting on you on top ---
Sleep 2.5s

# --- 2. jump straight to an agent that needs you ---
Type "g"
Sleep 2s

# --- 3. babysit grid: four agents live at once (the money shot) ---
Space                 # fleet → babysit grid
Sleep 3s
Right                 # move across the live tiles
Sleep 1s
Right
Sleep 1s
Right
Sleep 1.5s
Escape                # grid → back to the fleet
Sleep 800ms

# --- 4. zoom in: fleet → split → focus (one agent full-screen) ---
Enter                 # fleet → split (selected agent's live output + sidebar)
Sleep 2.5s
Right                 # split → focus
Sleep 3s

# --- back out, then quit ---
Escape                # focus → split
Sleep 800ms
Escape                # split → fleet
Sleep 1.5s
Type "q"              # quit
Sleep 1s
