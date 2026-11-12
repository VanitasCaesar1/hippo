// Draws an SVG into a square PNG with an alpha channel, so the corners outside
// the icon's rounded square stay transparent (qlmanage paints them white).
//
//   swift render-icon.swift <input.svg> <output.png> <pixels>
//
// Used by icon.sh. AppKit's NSImage reads the SVG, so no image tool is needed.
import AppKit

let args = CommandLine.arguments
guard args.count == 4, let pixels = Int(args[3]), pixels > 0 else {
    FileHandle.standardError.write(Data("usage: render-icon.swift <input.svg> <output.png> <pixels>\n".utf8))
    exit(2)
}
guard let image = NSImage(contentsOfFile: args[1]) else {
    FileHandle.standardError.write(Data("cannot read \(args[1])\n".utf8))
    exit(1)
}
guard let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
else { exit(1) }

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
image.draw(in: NSRect(x: 0, y: 0, width: pixels, height: pixels))
NSGraphicsContext.restoreGraphicsState()

guard let png = rep.representation(using: .png, properties: [:]) else { exit(1) }
do {
    try png.write(to: URL(fileURLWithPath: args[2]))
} catch {
    FileHandle.standardError.write(Data("cannot write \(args[2]): \(error)\n".utf8))
    exit(1)
}
