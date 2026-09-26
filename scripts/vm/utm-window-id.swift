import CoreGraphics
import Foundation
let list = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list where (w[kCGWindowOwnerName as String] as? String) == "UTM" {
    let b = w[kCGWindowBounds as String] as? [String: Any] ?? [:]
    print(w[kCGWindowNumber as String] ?? "", w[kCGWindowName as String] ?? "", b["Width"] ?? "", b["Height"] ?? "", w[kCGWindowLayer as String] ?? "")
}
