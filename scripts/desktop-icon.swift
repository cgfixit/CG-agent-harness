// Native vector drawing used to reproduce the application icon; no external art.
import AppKit
import Foundation

let destination = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
for (size, name) in [(16,"16x16"),(32,"16x16@2x"),(32,"32x32"),(64,"32x32@2x"),(128,"128x128"),(256,"128x128@2x"),(256,"256x256"),(512,"256x256@2x"),(512,"512x512"),(1024,"512x512@2x")] {
    let image = NSImage(size: NSSize(width: size, height: size))
    image.lockFocus()
    let s = CGFloat(size)
    NSColor(red: 0.06, green: 0.10, blue: 0.14, alpha: 1).setFill()
    NSBezierPath(roundedRect: NSRect(x: 0, y: 0, width: s, height: s), xRadius: s * 0.21, yRadius: s * 0.21).fill()
    NSColor(red: 0.44, green: 0.88, blue: 0.74, alpha: 1).setStroke()
    let path = NSBezierPath()
    path.lineWidth = s * 0.055
    path.lineCapStyle = .round
    path.move(to: NSPoint(x: s * 0.30, y: s * 0.68))
    path.line(to: NSPoint(x: s * 0.13, y: s * 0.50))
    path.line(to: NSPoint(x: s * 0.30, y: s * 0.32))
    path.move(to: NSPoint(x: s * 0.70, y: s * 0.68))
    path.line(to: NSPoint(x: s * 0.87, y: s * 0.50))
    path.line(to: NSPoint(x: s * 0.70, y: s * 0.32))
    path.stroke()
    let text = "CG" as NSString
    let attributes: [NSAttributedString.Key: Any] = [
        .font: NSFont.monospacedSystemFont(ofSize: s * 0.25, weight: .bold),
        .foregroundColor: NSColor.white
    ]
    let bounds = text.size(withAttributes: attributes)
    text.draw(at: NSPoint(x: (s - bounds.width) / 2, y: (s - bounds.height) / 2), withAttributes: attributes)
    image.unlockFocus()
    guard let tiff = image.tiffRepresentation,
          let bitmap = NSBitmapImageRep(data: tiff),
          let png = bitmap.representation(using: .png, properties: [:]) else { fatalError("icon drawing failed") }
    try png.write(to: destination.appendingPathComponent("icon_\(name).png"))
}
