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
print(window[kCGWindowNumber as String]!)
