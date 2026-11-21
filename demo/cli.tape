# Command mode. Record with: vhs demo/cli.tape
Output demo/cli.gif
Require meili

Set Shell bash
Set FontSize 14
Set Width 1440
Set Height 460
Set Padding 16
Set TypingSpeed 60ms

Env HOME "/tmp/ada"

Hide
Type "cd $(mktemp -d) && clear"
Enter
Show

Type "meili --demo mail list --limit 5"
Sleep 400ms
Enter
Sleep 3s

Type "meili --demo mail list --query 'from:kestrel has:attachment'"
Sleep 400ms
Enter
Sleep 3s

Type "meili --demo mail read 19a6f0c4b2e00002"
Sleep 400ms
Enter
Sleep 4s

Type "clear"
Enter
Type "meili --demo mail attachments --query 'from:orchard' --out reports"
Sleep 400ms
Enter
Sleep 3s

Type "meili --demo mail delete --label newsletters"
Sleep 400ms
Enter
Sleep 2500ms

Type "echo 'See you Thursday.' | meili --demo mail send --reply-to 19a6f0c4b2e0000c --body -"
Sleep 400ms
Enter
Sleep 3s
