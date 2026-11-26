$e = 'C:\CLLL\.calisma\vw-avatar-yeni\target\diag\victory.exe'
Remove-NetFirewallRule -DisplayName 'victory-fork engel 8 giden' -ErrorAction SilentlyContinue
Remove-NetFirewallRule -DisplayName 'victory-fork engel 8 gelen' -ErrorAction SilentlyContinue
New-NetFirewallRule -DisplayName 'victory-fork engel 8 giden' -Direction Outbound -Program $e -Action Block -Profile Any | Out-Null
New-NetFirewallRule -DisplayName 'victory-fork engel 8 gelen' -Direction Inbound -Program $e -Action Block -Profile Any | Out-Null