# Labels, search, jumping between labels, archive and trash. Record with: vhs demo/organize.tape
Output demo/organize.gif
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

Type "l"
Sleep 1200ms
Type@400ms "jjjjjj"
Sleep 600ms
Enter
Sleep 1500ms
Escape
Sleep 800ms

Type "/"
Sleep 400ms
Type@90ms "from:kestrel has:attachment"
Sleep 600ms
Enter
Sleep 2500ms
Escape
Sleep 1s

Type ":"
Sleep 600ms
Type@250ms "inv"
Sleep 1s
Enter
Sleep 2s
Tab
Sleep 1500ms

Type "a"
Sleep 1200ms
Type "d"
Sleep 1200ms
Type "D"
Sleep 1500ms
Type "y"
Sleep 2s
Type "q"
Sleep 500ms
