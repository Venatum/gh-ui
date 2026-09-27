# The README's GIF. Run through ./demo/record.sh, which sets DEMO_PATH,
# DEMO_HOME and DEMO_WORKSPACE: running `vhs demo/demo.tape` directly would
# show the real account and the real repos.

Output assets/demo.gif

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 780
Set Padding 12
Set Theme "Catppuccin Mocha"
Set TypingSpeed 80ms

Hide
Type `export PATH="$DEMO_PATH" HOME="$DEMO_HOME" PS1="$ " && cd "$DEMO_WORKSPACE" && clear`
Enter
Show

Type "gh-ui"
Sleep 400ms
Enter
Sleep 2.5s

# PRs: move around, search, then only mine.
Down 3
Sleep 800ms
Type "/"
Sleep 400ms
Type "fix"
Sleep 1.5s
Enter
Sleep 1s
Escape
Sleep 800ms
Type "m"
Sleep 2s
Type "m"
Sleep 1s

# Actions: the runs of those PRs.
Type "2"
Sleep 2.5s

# Issues: the ones assigned to me, from the filters panel.
Type "3"
Sleep 2s
Type "f"
Sleep 800ms
Down 2
Sleep 500ms
Right
Sleep 1.5s
Type "f"
Sleep 2s

# Repos: my public repos, then an org's, with what is already cloned.
Type "4"
Sleep 2.5s
Type "f"
Sleep 800ms
Right
Sleep 1.2s
Down
Right
Sleep 600ms
Down
Right
Sleep 1s
Type "f"
Sleep 3s

# Quit off camera: the GIF loops back from the Repos tab, not a shell.
Hide
Type "q"
Sleep 500ms
