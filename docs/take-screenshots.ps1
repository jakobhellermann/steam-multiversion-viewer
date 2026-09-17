# PowerShell script: positioniert, fokussiert und screenshotet das Steam Multiversion Viewer Fenster.
# Captured wird der Client-Bereich (ohne Fensterrahmen), Mica wird durch Fokus aktiviert.

param(
    [string]$WindowTitle = "Steam Multiversion Viewer",
    [string]$OutputPath = "screenshot.png",
    [int]$X = -1600,
    [int]$Y = 90,
    [int]$Width = 1600,
    [int]$Height = 1000,
    [int]$TimeoutMs = 10000
)

$source = @"
using System;
using System.Runtime.InteropServices;

public class User32 {
    [DllImport("user32.dll", SetLastError = true)]
    public static extern IntPtr FindWindow(string lpClassName, string lpWindowName);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool SetWindowPos(IntPtr hWnd, IntPtr hWndInsertAfter, int X, int Y, int cx, int cy, uint uFlags);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, int nFlags);
    [DllImport("user32.dll")]
    public static extern IntPtr GetDC(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern int ReleaseDC(IntPtr hWnd, IntPtr hDC);
    [DllImport("user32.dll")]
    public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")]
    public static extern bool GetClientRect(IntPtr hWnd, out RECT lpRect);
    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint lpdwProcessId);
    [DllImport("kernel32.dll")]
    public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")]
    public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool ClientToScreen(IntPtr hWnd, ref POINT lpPoint);
}

public class Gdi32 {
    [DllImport("gdi32.dll")]
    public static extern IntPtr CreateCompatibleDC(IntPtr hdc);
    [DllImport("gdi32.dll")]
    public static extern IntPtr CreateCompatibleBitmap(IntPtr hdc, int nWidth, int nHeight);
    [DllImport("gdi32.dll")]
    public static extern IntPtr SelectObject(IntPtr hdc, IntPtr hgdiobj);
    [DllImport("gdi32.dll")]
    public static extern bool BitBlt(IntPtr hdcDest, int nXDest, int nYDest, int nWidth, int nHeight,
        IntPtr hdcSrc, int nXSrc, int nYSrc, uint dwRop);
    [DllImport("gdi32.dll")]
    public static extern bool DeleteObject(IntPtr hObject);
    [DllImport("gdi32.dll")]
    public static extern bool DeleteDC(IntPtr hdc);
}

public struct RECT {
    public int Left; public int Top; public int Right; public int Bottom;
}
public struct POINT {
    public int X; public int Y;
}
"@

Add-Type -TypeDefinition $source

function Get-WindowHandle
{
    param([string]$Title)
    $proc = Get-Process | Where-Object { $_.MainWindowTitle -eq $Title -and $_.ProcessName -like '*steam*multiversion*' } | Select-Object -First 1
    if ($proc)
    { $proc.MainWindowHandle
    } else
    { [User32]::FindWindow($null, $Title)
    }
}

function Wait-ForWindow
{
    param([string]$Title, [int]$TimeoutMs)
    $elapsed = 0
    $interval = 100
    while ($elapsed -lt $TimeoutMs)
    {
        $hwnd = Get-WindowHandle -Title $Title
        if ($hwnd -ne [IntPtr]::Zero)
        { return $hwnd
        }
        Start-Sleep -Milliseconds $interval
        $elapsed += $interval
    }
    [IntPtr]::Zero
}

function Get-WindowRect
{
    param([IntPtr]$hWnd)
    $rect = New-Object RECT
    $ok = [User32]::GetWindowRect($hWnd, [ref]$rect)
    if ($ok)
    {
        return [PSCustomObject]@{ Left=$rect.Left; Top=$rect.Top; Width=$rect.Right-$rect.Left; Height=$rect.Bottom-$rect.Top }
    }
    $null
}

# --- Main ---
Write-Host "Looking for window: $WindowTitle (timeout: ${TimeoutMs}ms)"
$hwnd = Wait-ForWindow -Title $WindowTitle -TimeoutMs $TimeoutMs

if ($hwnd -eq [IntPtr]::Zero)
{
    Write-Error "Window '$WindowTitle' not found within ${TimeoutMs}ms timeout"
    exit 1
}
Write-Host "Found window handle: 0x$($hwnd.ToString('X'))"

# Position/size window
Write-Host "Positioning window to (${X}, ${Y}) with size ${Width}x${Height}"
$flags = 0x0004 -bor 0x0010  # SWP_NOZORDER | SWP_NOACTIVATE
[User32]::SetWindowPos($hwnd, [IntPtr]::Zero, $X, $Y, $Width, $Height, $flags) | Out-Null
Start-Sleep -Milliseconds 100

# Focus window (AttachThreadInput trick to bypass foreground lock)
$foreHwnd = [User32]::GetForegroundWindow()
$foreThreadId = [uint32]0; $targetThreadId = [uint32]0
$ourThreadId = [User32]::GetCurrentThreadId()
[User32]::GetWindowThreadProcessId($foreHwnd, [ref]$foreThreadId) | Out-Null
[User32]::GetWindowThreadProcessId($hwnd, [ref]$targetThreadId) | Out-Null
if ($foreThreadId -ne $ourThreadId)
{ [User32]::AttachThreadInput($ourThreadId, $foreThreadId, $true) | Out-Null
}
if ($targetThreadId -ne $ourThreadId)
{ [User32]::AttachThreadInput($ourThreadId, $targetThreadId, $true) | Out-Null
}
[User32]::SetForegroundWindow($hwnd) | Out-Null
if ($foreThreadId -ne $ourThreadId)
{ [User32]::AttachThreadInput($ourThreadId, $foreThreadId, $false) | Out-Null
}
if ($targetThreadId -ne $ourThreadId)
{ [User32]::AttachThreadInput($ourThreadId, $targetThreadId, $false) | Out-Null
}
Write-Host "Window focused (foreground)"

Start-Sleep -Milliseconds 300

# Capture full window via PrintWindow + crop to client rect
$winRect = Get-WindowRect -hWnd $hwnd
$clientRect = New-Object RECT
[User32]::GetClientRect($hwnd, [ref]$clientRect) | Out-Null

$pt = New-Object POINT
$pt.X = 0; $pt.Y = 0
[User32]::ClientToScreen($hwnd, [ref]$pt) | Out-Null

# Frame inset = client origin on screen minus window origin
$frameLeft = $pt.X - $winRect.Left
$frameTop = $pt.Y - $winRect.Top
$clientW = $clientRect.Right - $clientRect.Left
$clientH = $clientRect.Bottom - $clientRect.Top

Write-Host "Window rect: ${Width}x${Height}, Client area: Position=($($pt.X), $($pt.Y)) Size=${clientW}x${clientH}, Frame inset: $frameLeft,$frameTop"

# Capture full window via PrintWindow (flag 2 = PW_RENDERFULLCONTENT)
$screenDC = [User32]::GetDC([IntPtr]::Zero)
$windowDC = [User32]::GetDC($hwnd)

$memDC = [Gdi32]::CreateCompatibleDC($screenDC)
$fullBmp = [Gdi32]::CreateCompatibleBitmap($screenDC, $winRect.Width, $winRect.Height)
[Gdi32]::SelectObject($memDC, $fullBmp) | Out-Null
[User32]::PrintWindow($hwnd, $memDC, 2) | Out-Null

# Convert full bitmap to System.Drawing Bitmap and crop client area
[void][Reflection.Assembly]::LoadWithPartialName("System.Drawing")
$fullImg = [System.Drawing.Image]::FromHbitmap($fullBmp)

$clientImg = $fullImg.Clone(
    [System.Drawing.Rectangle]::new($frameLeft, $frameTop, $clientW, $clientH),
    [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
)
$clientImg.Save($OutputPath, [System.Drawing.Imaging.ImageFormat]::Png)

$clientImg.Dispose()
$fullImg.Dispose()

[Gdi32]::DeleteObject($fullBmp) | Out-Null
[Gdi32]::DeleteDC($memDC) | Out-Null
[User32]::ReleaseDC([IntPtr]::Zero, $screenDC) | Out-Null
[User32]::ReleaseDC($hwnd, $windowDC) | Out-Null

Write-Host "Screenshot saved to: $OutputPath (client area ${clientW}x${clientH})"
Write-Host "Done!"
