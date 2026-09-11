# Saved queries: one made in the Snippets tab as a line, then shaped in vim with
# `o`, ready for `esql <connection> :name`. SQL goes in at paste speed: nobody
# watches a query being typed.
#
#   cd demo && ./stage.sh up && vhs snippets.tape && ./stage.sh down

Output "../readme-assets/snippets.gif"

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

Sleep 1s
Tab
Sleep 300ms
Tab
Sleep 300ms
Tab
Sleep 2s

Type "c"
Sleep 1.2s
Type "running"
Enter
Sleep 500ms
Type@2ms "select pid, query from pg_stat_activity where state = 'active'"
Sleep 1s
Enter
Sleep 2.5s

# --- `o` opens the file in $EDITOR, where a longer query belongs ------------
Type "o"
Sleep 1.5s
# paste mode, so vim's indenting cannot rearrange what goes in
Type ":set paste"
Enter
Type "ggdG"
Type "i"
Type@1ms "select pid,"
Enter
Type@1ms "       now() - query_start as running_for,"
Enter
Type@1ms "       left(query, 60)     as query"
Enter
Type@1ms "  from pg_stat_activity"
Enter
Type@1ms " where state = 'active'"
Enter
Type@1ms " order by running_for desc;"
Sleep 1.5s
Escape
Type ":wq"
Enter
Sleep 3.5s

Hide
Type "q"
Sleep 1s
