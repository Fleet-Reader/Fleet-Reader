# Run against an English windows-winui Fleet Reader with its sidebar expanded.
# Uses real mouse input: scripted navigation and UIA selection bypass this path.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class NewsSidebarInput {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
}
'@
$app = Get-Process day-news | Select-Object -First 1
if (!$app) { throw 'Launch Fleet Reader with --locale en first.' }
$window = [System.Windows.Automation.AutomationElement]::RootElement.FindFirst(
    [System.Windows.Automation.TreeScope]::Children,
    [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ProcessIdProperty, $app.Id))
function Find-Id($Id) {
    $element = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::AutomationIdProperty, $Id))
    if (!$element) { throw "Missing native element: $Id" }
    return $element
}
[NewsSidebarInput]::SetForegroundWindow($app.MainWindowHandle) | Out-Null
Start-Sleep -Milliseconds 300
# Visit each destination twice, clicking its text and then the row's padding.
foreach ($padding in @($false, $true)) {
    foreach ($name in @('All Articles', 'Starred', 'All Unread', 'Today')) {
        $sidebar = Find-Id 'nav-reorder-list'
        $label = $sidebar.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
            [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, $name))
        if (!$label) { throw "Missing sidebar destination: $name" }
        $row = $label
        while ($row -and $row.Current.ClassName -notmatch '(^|\.)ItemContainer$') {
            $row = [System.Windows.Automation.TreeWalker]::RawViewWalker.GetParent($row)
        }
        if (!$row) { throw "$name is not hosted in a native recyclable row" }
        $bounds = $label.Current.BoundingRectangle
        $x = $bounds.Left + $bounds.Width / 2
        if ($padding) { $x = $row.Current.BoundingRectangle.Left + 3 }
        if ($row.Current.IsOffscreen -or $bounds.IsEmpty) { throw "$name is not visible" }
        [NewsSidebarInput]::SetCursorPos([int]$x, [int]($bounds.Top + $bounds.Height / 2)) | Out-Null
        Start-Sleep -Milliseconds 100
        [NewsSidebarInput]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 80
        [NewsSidebarInput]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
        $deadline = [DateTime]::UtcNow.AddSeconds(5)
        do {
            Start-Sleep -Milliseconds 100
            $actual = (Find-Id 'scope-title').Current.Name
        } while ($actual -ne $name -and [DateTime]::UtcNow -lt $deadline)
        if ($actual -ne $name) { throw "Clicking '$name' (padding=$padding) left scope-title at '$actual'" }
        Write-Output "PASS: mouse click '$name' (padding=$padding)"
    }
}
