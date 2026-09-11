# Browse and connect: the list, a connection whose tunnel is not running, Enter
# reopening it into a real session, and that forward on the Tunnels tab.
#
#   cd demo && ./stage.sh up && vhs browse.tape && ./stage.sh down

Output "../readme-assets/browse.gif"

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

Sleep 2s

# --- down the list: each row's state and where it points --------------------
Down
Sleep 800ms
Down
Sleep 800ms
Down
Sleep 800ms
Down
Sleep 3s

# --- Enter reopens the tunnel and hands over a real psql --------------------
Enter
Sleep 3.5s
# SQL goes in at paste speed: nobody watches a query being typed.
Type@2ms "select region, total from sales order by total desc;"
Enter
Sleep 3s
Type "\q"
Enter
Sleep 2.5s

# --- the forward it opened, on the Tunnels tab, and off again ---------------
Tab
Sleep 400ms
Tab
Sleep 2s
Down
Sleep 400ms
Down
Sleep 1.5s
Enter
Sleep 3s

Hide
Type "q"
Sleep 1s
