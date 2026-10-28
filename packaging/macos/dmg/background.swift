import AppKit
// The DMG window background: 640x400pt, light, an arrow from the app to
// Applications and a one-line instruction.
let out = CommandLine.arguments[1]
let scale = Double(CommandLine.arguments[2])!
let (W, H) = (640.0, 400.0)
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(W*scale), pixelsHigh: Int(H*scale), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = NSSize(width: W, height: H)
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
func c(_ hex: UInt32, _ a: Double = 1) -> NSColor { NSColor(srgbRed: Double((hex>>16)&255)/255, green: Double((hex>>8)&255)/255, blue: Double(hex&255)/255, alpha: a) }
c(0xF5F6F8).setFill(); NSRect(x: 0, y: 0, width: W, height: H).fill()
// A soft ring glow behind, in the logo colours.
let ctx = NSGraphicsContext.current!.cgContext
let glow = NSGradient(colors: [c(0x4A6CFF, 0.10), c(0x2A9BA3, 0.06), c(0xF5F6F8, 0)], atLocations: [0, 0.5, 1], colorSpace: .sRGB)!
glow.draw(fromCenter: NSPoint(x: W/2, y: H-200), radius: 0, toCenter: NSPoint(x: W/2, y: H-200), radius: 330, options: [])
// Arrow from the app (x 170) to Applications (x 470), icons centred at y 190 from the top.
let y = H - 190
let arrow = NSBezierPath()
arrow.move(to: NSPoint(x: 262, y: y)); arrow.line(to: NSPoint(x: 372, y: y))
arrow.lineWidth = 3; arrow.lineCapStyle = .round
let grad = NSGradient(starting: c(0x4A6CFF), ending: c(0x45C75B))!
ctx.saveGState()
ctx.setLineWidth(3); ctx.setLineCap(.round); ctx.setLineJoin(.round)
let p = CGMutablePath()
p.move(to: CGPoint(x: 262, y: y)); p.addLine(to: CGPoint(x: 376, y: y))
p.move(to: CGPoint(x: 362, y: y + 12)); p.addLine(to: CGPoint(x: 377, y: y)); p.addLine(to: CGPoint(x: 362, y: y - 12))
ctx.addPath(p); ctx.replacePathWithStrokedPath(); ctx.clip()
grad.draw(in: NSRect(x: 258, y: y - 14, width: 124, height: 28), angle: 0)
ctx.restoreGState()
// Instruction.
let para = NSMutableParagraphStyle(); para.alignment = .center
let text = NSAttributedString(string: "Drag Circle to Applications to install", attributes: [
  .font: NSFont.systemFont(ofSize: 15, weight: .medium), .foregroundColor: c(0x5A6170), .paragraphStyle: para])
text.draw(in: NSRect(x: 0, y: 70, width: W, height: 24))
NSGraphicsContext.current = nil
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
