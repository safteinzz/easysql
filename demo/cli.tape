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
Type "./stage.sh shell"
Enter
Sleep 2s
Show

Type "esql ls -v"
Enter
Sleep 2s
Screenshot ../readme-assets/ls.png
Sleep 1s
