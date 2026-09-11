# Passwords: saved from the connection that lacks one, into the file psql reads,
# and listed as dots.
#
#   cd demo && ./stage.sh up && vhs passwords.tape && ./stage.sh down

Output "../readme-assets/passwords.gif"

Set Shell bash
Set Theme "Catppuccin Mocha"
Set FontFamily "JetBrainsMono NF"
Set FontSize 15
Set Width 1320
Set Height 640
Set Padding 20
Set Framerate 24
Set TypingSpeed 55ms

Hide
# `exec`, so a stage that refuses to open takes the terminal with it: without it
# the script exits, the outer shell survives, and every keystroke after this
# lands in the *real* home.
Type "exec ./stage.sh shell"
Enter
Sleep 2s
# The first frame is what the README shows before the GIF plays, so it opens on
# the toolbox, not on a shell prompt.
Type "esql"
Enter
Sleep 3s
Show

Sleep 1.5s

# --- `metrics` is the one with no password on file --------------------------
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 1.5s
Type "p"
Sleep 1.5s
Tab 4
Sleep 500ms
Type "devpass"
Sleep 800ms
Enter
Sleep 2.5s

# --- and there it is, never shown -------------------------------------------
Tab
Sleep 3s

Hide
Type "q"
Sleep 1s
