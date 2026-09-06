import CoreGraphics
import Foundation
let pid = Int32(CommandLine.arguments[1])!
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as! [[String: Any]]
let candidates = windows.filter { ($0[kCGWindowOwnerPID as String] as? Int32) == pid && ($0[kCGWindowLayer as String] as? Int) == 0 }
let window = candidates.max { a, b in
    let aa = a[kCGWindowBounds as String] as! [String: Double]
    let bb = b[kCGWindowBounds as String] as! [String: Double]
    return aa["Width"]! * aa["Height"]! < bb["Width"]! * bb["Height"]!
}
guard let window = window else { fputs("Demo window not found\n", stderr); exit(1) }
if CommandLine.arguments.contains("--park-cursor") {
    let display = CGDisplayBounds(CGMainDisplayID())
    CGWarpMouseCursorPosition(CGPoint(x: display.maxX - 2, y: display.maxY - 2))
    Thread.sleep(forTimeInterval: 0.6)
}
if CommandLine.arguments.contains("--json") {
    let bounds = window[kCGWindowBounds as String] as! [String: Double]
    let result: [String: Any] = ["id": window[kCGWindowNumber as String]!,
                               "pid": pid, "width": bounds["Width"]!, "height": bounds["Height"]!]
    let data = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
} else {
    print(window[kCGWindowNumber as String]!)
}
