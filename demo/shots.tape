# Every still in the README, rendered straight into ../readme-assets/.
#
#   cd demo && ./stage.sh up && vhs shots.tape && ./stage.sh down
#
# One tape at a time: two tapes sharing the staged HOME delete each other's
# fixtures mid-take, and the symptom is identical images rather than an error.

Output "/tmp/easysql-shots.mp4"

Set Shell bash
Set Theme "Catppuccin Mocha"
Set FontFamily "JetBrainsMono NF"
Set FontSize 15
Set Width 1320
Set Height 640
Set Padding 20
Set Framerate 24
Set TypingSpeed 55ms

# vhs runs the tape from the directory it was invoked in, which is `demo/`, so
# this is a bare `./stage.sh shell` - and that script refuses to open one unless
# the stage exists, so a broken setup stops the render instead of quietly
# screenshotting the real home.
Hide
Type "./stage.sh shell"
Enter
Sleep 2s
Show

# --- the Connections list, which is what you see first ----------------------
# Up, sleeping-tunnel and unreachable all in one frame, with the detail panel
# open on the connection the rest of the README talks about.
Type "esql"
Enter
Sleep 3s
Screenshot ../readme-assets/connections.png
Sleep 700ms

# --- the detail panel on a connection that needs a tunnel -------------------
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 2.5s
Screenshot ../readme-assets/tunnel-aware.png
Sleep 700ms

# --- the connection wizard, showing the command it will run -----------------
Escape
Sleep 300ms
Type "e"
Sleep 1.5s
Screenshot ../readme-assets/wizard.png
Sleep 700ms
Escape
Sleep 500ms

# --- the Passwords tab ------------------------------------------------------
Tab
Sleep 2s
Screenshot ../readme-assets/passwords.png
Sleep 700ms

# --- the Tunnels tab, each forward naming the connection it serves ----------
Tab
Sleep 2s
Screenshot ../readme-assets/tunnels.png
Sleep 700ms

# --- the Snippets tab, and what `:name` runs --------------------------------
Tab
Sleep 2s
Screenshot ../readme-assets/snippets.png
Sleep 700ms

# --- the Settings tab -------------------------------------------------------
Tab
Sleep 2s
Screenshot ../readme-assets/settings.png
Sleep 700ms

Type "q"
Sleep 800ms
