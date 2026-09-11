# Edit a connection: the preview following the fields, then Read only flipped
# and the name turning blue.
#
#   cd demo && ./stage.sh up && vhs edit.tape && ./stage.sh down

Output "../readme-assets/edit.gif"

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
Type "e"
Sleep 1.5s

# --- the preview follows the database as it is typed ------------------------
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 800ms
Backspace 3
Type "shop"
Sleep 2s
Backspace 4
Type "app"
Sleep 1s

# --- Read only, then save ----------------------------------------------------
# Three rows down from Database: User, Encryption, Read only.
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 800ms
Right
Sleep 1.5s
Enter
Sleep 3.5s

Hide
Type "q"
Sleep 1s
