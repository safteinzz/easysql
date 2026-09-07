# The CLI stills. Its own tape because a terminal frame is sized to what is in
# it, and the TUI's 640px pane would leave this one four fifths empty.
#
#   cd demo && ./stage.sh up && vhs cli.tape && ./stage.sh down

Output "/tmp/easysql-cli.mp4"

Set Shell bash
Set Theme "Catppuccin Mocha"
Set FontFamily "JetBrainsMono NF"
Set FontSize 15
Set Width 1320
Set Height 300
Set Padding 20
Set Framerate 24
Set TypingSpeed 55ms

Hide
# `exec`, so a stage that refuses to open takes the terminal with it: without it
# the script exits, the outer shell survives, and every keystroke after this
# lands in the *real* home - which is how a README image ends up showing
# somebody's actual connections.
Type "exec ./stage.sh shell"
Enter
Sleep 2s
Show

Type "esql ls -v"
Enter
Sleep 2s
Screenshot ../readme-assets/ls.png
Sleep 1s

# The saved query, run against a connection from the shell. Same word on any
# engine, which is the whole point and is invisible in a still of the tab.
Type "clear"
Enter
Sleep 300ms
Type "esql app :tables"
Enter
Sleep 2.5s
Screenshot ../readme-assets/snippet-run.png
Sleep 1s
