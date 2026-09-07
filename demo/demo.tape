# The tour at the top of the README: what a new user does, in order.
#
#   cd demo && ./stage.sh up && vhs demo.tape && ./stage.sh down
#
# One beat per feature, and every beat finishes: make the thing, see the result,
# put it back. Results hold for a couple of seconds, keystrokes hurry.

Output "../readme-assets/demo.gif"

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
# lands in the *real* home - which is how a README image ends up showing
# somebody's actual connections.
Type "exec ./stage.sh shell"
Enter
Sleep 2s
Show

# --- beat 1: what you type, and what you get --------------------------------
Type "esql"
Enter
Sleep 3.5s

# --- beat 2: open a real session, and come back -----------------------------
# The payoff first: Enter is a real psql against a real server, and `\q` puts
# you back in the list exactly where you were.
Enter
Sleep 3s
Type "select name, email from customer order by id limit 3;"
Enter
Sleep 3s
Type "\q"
Enter
Sleep 2.5s

# --- beat 3: a connection that only works through a tunnel ------------------
# `warehouse` is yellow: its forward is not running. The panel says so, and
# says what will bring it back.
Down
Sleep 300ms
Down
Sleep 300ms
Down
Sleep 3s

# --- beat 4: the wizard writes the file the client already reads ------------
Type "e"
Sleep 2s
Down
Sleep 400ms
Down
Sleep 400ms
Down
Sleep 2.5s
Escape
Sleep 1.5s

# --- beat 5: find, then drop the filter -------------------------------------
Type "/"
Sleep 500ms
Type "sh"
Sleep 2s
Escape
Sleep 1.5s

# --- beat 6: passwords go where the client looks for them -------------------
Tab
Sleep 3s

# --- beat 7: tunnels, each one naming what it is for ------------------------
Tab
Sleep 3s

# --- beat 8: snippets, the saved queries `:name` runs ------------------------
Tab
Sleep 3.5s

# --- beat 9: settings, and back to where we started -------------------------
Tab
Sleep 3s
Tab
Sleep 2s

Type "q"
Sleep 2s
