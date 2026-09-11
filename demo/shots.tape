# The TUI's one still in the README, Settings, rendered into ../readme-assets/.
# Every screen you act on is a GIF of its own tape instead; a still is kept for
# a screen that shows everything at once.
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
# `exec`, so a stage that refuses to open takes the terminal with it: without it
# the script exits, the outer shell survives, and every keystroke after this
# lands in the *real* home - which is how a README image ends up showing
# somebody's actual connections.
Type "exec ./stage.sh shell"
Enter
Sleep 2s
Show

Type "esql"
Enter
Sleep 3s
Tab
Sleep 300ms
Tab
Sleep 300ms
Tab
Sleep 300ms
# --- the Settings tab -------------------------------------------------------
Tab
Sleep 2s
Screenshot ../readme-assets/settings.png
Sleep 700ms

Type "q"
Sleep 800ms
