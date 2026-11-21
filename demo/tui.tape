# Reading mail and saving an attachment. Record with: vhs demo/tui.tape
Output demo/tui.gif
Require meili

Set Shell bash
Set FontSize 16
Set Width 1280
Set Height 660
Set Padding 16
Set TypingSpeed 60ms

Env HOME "/tmp/ada"

Type "meili --demo"
Sleep 500ms
Enter
Sleep 2s

Type@450ms "jjjjj"
Type@450ms "kkkk"
Sleep 1s
Enter
Sleep 3s
Type@300ms "jjj"
Sleep 1s

Type "o"
Sleep 1500ms
Type "j"
Sleep 800ms
Type "s"
Sleep 2500ms
Escape
Sleep 800ms
Type "q"
Sleep 1s
Type "q"
Sleep 500ms
