#!/usr/bin/env swift
// macOS-only, dependency-free export of the approved generated Kiri stencils.
// Run from the repository root: swift scripts/export_brand.swift
import AppKit
import CoreText
import Foundation

let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
let brand = root.appendingPathComponent("assets/brand")
let fm = FileManager.default
let space = CGColorSpace(name: CGColorSpace.sRGB)!
let sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 160, 192, 256, 512, 1024]

func write(_ data: Data, _ url: URL) throws {
    try fm.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
    try data.write(to: url, options: .atomic)
}
func write(_ text: String, _ url: URL) throws { try write(Data(text.utf8), url) }
func color(_ hex: UInt32) -> CGColor {
    CGColor(colorSpace: space, components: [CGFloat((hex >> 16) & 255)/255, CGFloat((hex >> 8) & 255)/255, CGFloat(hex & 255)/255, 1])!
}
func number(_ x: CGFloat) -> String { String(format: "%.3f", Double(x)) }
func transformed(_ path: CGPath, _ t: CGAffineTransform) -> CGPath {
    var transform = t
    return path.copy(using: &transform)!
}

// Trace the foreground into merged rectangles. No embedded raster in the SVGs.
// The source is a two-tone generated stencil; 0.8 separates white from black.
func trace(_ url: URL) throws -> CGPath {
    guard let b = NSBitmapImageRep(data: try Data(contentsOf: url)) else { throw Failure("Cannot decode \(url.path)") }
    guard b.bitsPerSample == 8, !b.isPlanar, b.samplesPerPixel >= 3,
          b.bitsPerPixel.isMultiple(of:8),
          let pixels = b.bitmapData else { throw Failure("Expected interleaved 8-bit RGB stencil") }
    // RGB images may use padded RGBX storage: 3 samples but 4 bytes per pixel.
    let pixelStride = b.bitsPerPixel / 8
    let colorOffset = b.bitmapFormat.contains(.alphaFirst) ? 1 : 0
    var rects: [CGRect] = []
    var active: [String: Int] = [:]
    for y in 0..<b.pixelsHigh {
        var runs: [(Int, Int)] = []
        var start: Int?
        for x in 0...b.pixelsWide {
            let foreground: Bool
            if x < b.pixelsWide {
                let p = y*b.bytesPerRow + x*pixelStride + colorOffset
                foreground = min(pixels[p],pixels[p+1],pixels[p+2]) > 204
            } else { foreground = false }
            if foreground && start == nil { start = x }
            if !foreground, let s = start { runs.append((s, x-s)); start = nil }
        }
        var next: [String: Int] = [:]
        for (x, width) in runs {
            let key = "\(x):\(width)"
            if let i = active[key] { rects[i].size.height += 1; next[key] = i }
            else { next[key] = rects.count; rects.append(CGRect(x:x, y:y, width:width, height:1)) }
        }
        active = next
    }
    let path = CGMutablePath()
    for rect in rects { path.addRect(rect) }
    let bounds = path.boundingBox
    guard bounds.width > 100, bounds.height > 100 else { throw Failure("Empty or invalid stencil") }
    let scale = 800 / max(bounds.width, bounds.height)
    return transformed(path, CGAffineTransform(a:scale, b:0, c:0, d:scale,
        tx:512-bounds.midX*scale, ty:512-bounds.midY*scale))
}

// Custom geometric lowercase wordmark; filled outlines, no font dependency.
func wordmark() -> CGPath {
    let lines = CGMutablePath()
    lines.move(to: CGPoint(x:12,y:15)); lines.addLine(to: CGPoint(x:12,y:135))
    lines.move(to: CGPoint(x:76,y:52)); lines.addLine(to: CGPoint(x:14,y:98))
    lines.move(to: CGPoint(x:41,y:79)); lines.addLine(to: CGPoint(x:82,y:135))
    lines.move(to: CGPoint(x:118,y:55)); lines.addLine(to: CGPoint(x:118,y:135))
    lines.move(to: CGPoint(x:164,y:55)); lines.addLine(to: CGPoint(x:164,y:135))
    lines.move(to: CGPoint(x:164,y:89))
    lines.addCurve(to: CGPoint(x:219,y:57), control1: CGPoint(x:171,y:58), control2: CGPoint(x:191,y:47))
    lines.move(to: CGPoint(x:254,y:55)); lines.addLine(to: CGPoint(x:254,y:135))
    let path = CGMutablePath()
    path.addPath(lines.copy(strokingWithWidth:17, lineCap:.square, lineJoin:.miter, miterLimit:3))
    path.addEllipse(in:CGRect(x:108,y:14,width:20,height:20))
    path.addEllipse(in:CGRect(x:244,y:14,width:20,height:20))
    return path
}

enum Paint: String { case silver, white, black }
struct Texture { let image: CGImage; let data: Data }
struct Art {
    let path: CGPath
    let width: CGFloat
    let height: CGFloat
    var paint: Paint = .silver
    var background: Bool = false
    var tile: Bool = false
    var texture: Texture? = nil
}
func logo(_ mark: CGPath, stacked: Bool, paint: Paint) -> Art {
    let path = CGMutablePath()
    if stacked {
        path.addPath(transformed(mark, CGAffineTransform(a:0.72,b:0,c:0,d:0.72,tx:143.36,ty:0)))
        path.addPath(transformed(wordmark(), CGAffineTransform(a:1.35,b:0,c:0,d:1.35,tx:326,ty:773)))
        return Art(path:path,width:1024,height:1024,paint:paint)
    }
    path.addPath(transformed(mark, CGAffineTransform(a:0.48,b:0,c:0,d:0.48,tx:0,ty:10)))
    path.addPath(transformed(wordmark(), CGAffineTransform(a:1.9,b:0,c:0,d:1.9,tx:556,ty:112)))
    return Art(path:path,width:1152,height:512,paint:paint)
}
func draw(_ art: Art, _ ctx: CGContext) {
    if art.background { ctx.setFillColor(color(0x111418)); ctx.fill(CGRect(x:0,y:0,width:art.width,height:art.height)) }
    if art.tile {
        let rect = CGRect(x:64,y:64,width:896,height:896)
        ctx.setFillColor(color(0x15191E)); ctx.addPath(CGPath(roundedRect:rect,cornerWidth:194,cornerHeight:194,transform:nil)); ctx.fillPath()
    }
    if let texture = art.texture {
        let rect = art.tile ? CGRect(x:64,y:64,width:896,height:896) : CGRect(x:0,y:0,width:art.width,height:art.height)
        ctx.saveGState()
        if art.tile { ctx.addPath(CGPath(roundedRect:rect,cornerWidth:194,cornerHeight:194,transform:nil)); ctx.clip() }
        ctx.translateBy(x:rect.minX,y:rect.maxY); ctx.scaleBy(x:1,y:-1)
        ctx.draw(texture.image,in:CGRect(origin:.zero,size:rect.size)); ctx.restoreGState()
    }
    if art.tile {
        ctx.setStrokeColor(color(0x30363D)); ctx.setLineWidth(2)
        ctx.addPath(CGPath(roundedRect:CGRect(x:65,y:65,width:894,height:894),cornerWidth:193,cornerHeight:193,transform:nil)); ctx.strokePath()
    }
    guard !art.path.isEmpty else { return }
    ctx.saveGState()
    ctx.addPath(art.path)
    if art.paint == .silver {
        ctx.clip()
        let gradient = CGGradient(colorsSpace:space, colors:[color(0xF9FAFB),color(0xBAC2CA),color(0xF0F3F5),color(0xA8B1BB)] as CFArray, locations:[0,0.38,0.56,1])!
        ctx.drawLinearGradient(gradient,start:CGPoint(x:0,y:0),end:CGPoint(x:art.width*0.35,y:art.height),options:[.drawsBeforeStartLocation,.drawsAfterEndLocation])
    } else { ctx.setFillColor(color(art.paint == .white ? 0xFFFFFF : 0x111418)); ctx.fillPath() }
    ctx.restoreGState()
}
func raster(_ art: Art, width: Int) -> CGImage {
    let height = Int((CGFloat(width)*art.height/art.width).rounded())
    let ctx = CGContext(data:nil,width:width,height:height,bitsPerComponent:8,bytesPerRow:width*4,space:space,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.translateBy(x:0,y:CGFloat(height)); ctx.scaleBy(x:CGFloat(width)/art.width,y:-CGFloat(height)/art.height)
    draw(art,ctx)
    return ctx.makeImage()!
}
func png(_ image: CGImage, _ url: URL) throws {
    let bitmap = NSBitmapImageRep(cgImage:image)
    try write(bitmap.representation(using:.png,properties:[:])!,url)
}
func svgPath(_ path: CGPath) -> String {
    var commands: [String] = []
    path.applyWithBlock { pointer in
        let e = pointer.pointee
        func p(_ i:Int) -> String { "\(number(e.points[i].x)) \(number(e.points[i].y))" }
        switch e.type {
        case .moveToPoint: commands.append("M"+p(0))
        case .addLineToPoint: commands.append("L"+p(0))
        case .addQuadCurveToPoint: commands.append("Q"+p(0)+" "+p(1))
        case .addCurveToPoint: commands.append("C"+p(0)+" "+p(1)+" "+p(2))
        case .closeSubpath: commands.append("Z")
        @unknown default: break
        }
    }
    return commands.joined(separator: " ")
}
func svg(_ art: Art, _ url: URL) throws {
    let gradient = "<defs><linearGradient id=\"silver\" x1=\"0\" y1=\"0\" x2=\"\(number(art.width*0.35))\" y2=\"\(number(art.height))\" gradientUnits=\"userSpaceOnUse\"><stop offset=\"0\" stop-color=\"#f9fafb\"/><stop offset=\"0.38\" stop-color=\"#bac2ca\"/><stop offset=\"0.56\" stop-color=\"#f0f3f5\"/><stop offset=\"1\" stop-color=\"#a8b1bb\"/></linearGradient></defs>"
    var text = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 \(number(art.width)) \(number(art.height))\" width=\"\(number(art.width))\" height=\"\(number(art.height))\" role=\"img\" aria-label=\"Kiri\">"
    if art.paint == .silver { text += gradient }
    if art.background { text += "<path fill=\"#111418\" d=\"M0 0H\(number(art.width))V\(number(art.height))H0Z\"/>" }
    if art.tile { text += "<rect x=\"64\" y=\"64\" width=\"896\" height=\"896\" rx=\"194\" fill=\"#15191e\" stroke=\"#30363d\" stroke-width=\"2\"/>" }
    if let texture = art.texture {
        let pos = art.tile ? "x=\"64\" y=\"64\" width=\"896\" height=\"896\"" : "x=\"0\" y=\"0\" width=\"\(number(art.width))\" height=\"\(number(art.height))\""
        if art.tile { text += "<defs><clipPath id=\"tile\"><rect x=\"64\" y=\"64\" width=\"896\" height=\"896\" rx=\"194\"/></clipPath></defs>" }
        text += "<image \(pos) \(art.tile ? "clip-path=\"url(#tile)\"" : "") href=\"data:image/png;base64,\(texture.data.base64EncodedString())\"/>"
    }
    let fill = art.paint == .silver ? "url(#silver)" : (art.paint == .white ? "#ffffff" : "#111418")
    text += "<path fill=\"\(fill)\" d=\"\(svgPath(art.path))\"/></svg>\n"
    try write(text,url)
}
func pdf(_ art: Art, _ url: URL) throws {
    try fm.createDirectory(at:url.deletingLastPathComponent(),withIntermediateDirectories:true)
    var box = CGRect(x:0,y:0,width:art.width,height:art.height)
    guard let ctx = CGContext(url as CFURL,mediaBox:&box,nil) else { throw Failure("PDF context failed") }
    ctx.beginPDFPage(nil); ctx.translateBy(x:0,y:art.height); ctx.scaleBy(x:1,y:-1)
    draw(art,ctx); ctx.endPDFPage(); ctx.closePDF()
}

// Optical rasterization below 48px. Preserve 1px fog detail using ordered coverage,
// instead of averaging all the little squares into gray haze.
func small(_ mark: CGPath, size: Int, background: Bool) -> CGImage {
    let art = Art(path:mark,width:1024,height:1024,paint:.white)
    let image = raster(art,width:size*8)
    let bytes = image.dataProvider!.data! as Data
    let stride = image.bytesPerRow
    let bayer = [[0,8,2,10],[12,4,14,6],[3,11,1,9],[15,7,13,5]]
    let ctx = CGContext(data:nil,width:size,height:size,bitsPerComponent:8,bytesPerRow:size*4,space:space,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
    let out = ctx.data!.assumingMemoryBound(to:UInt8.self)
    for y in 0..<size { for x in 0..<size {
        var coverage = 0.0
        for sy in 0..<8 { for sx in 0..<8 {
            let rowOffset = (y*8+sy)*stride
            let columnOffset = (x*8+sx)*4
            coverage += Double(bytes[rowOffset+columnOffset+3])/255.0
        } }
        coverage /= 64
        let threshold = (Double(bayer[y%4][x%4])+0.5)/16
        let on = coverage > threshold
        let i = (y*size+x)*4
        out[i] = on ? 240 : (background ? 17 : 0)
        out[i+1] = on ? 243 : (background ? 20 : 0)
        out[i+2] = on ? 245 : (background ? 24 : 0)
        out[i+3] = on || background ? 255 : 0
    } }
    return ctx.makeImage()!
}

func pixelPath(_ image: CGImage) -> CGPath {
    let data = image.dataProvider!.data! as Data
    let path = CGMutablePath()
    let scale = 1024.0 / CGFloat(image.width)
    for y in 0..<image.height { for x in 0..<image.width {
        if data[y*image.bytesPerRow+x*4+3] > 0 {
            path.addRect(CGRect(x:CGFloat(x)*scale,y:CGFloat(y)*scale,width:scale,height:scale))
        }
    } }
    return path
}
func favicon(_ mark: CGPath, _ url: URL) throws {
    let p16 = pixelPath(small(mark,size:16,background:false))
    let p32 = pixelPath(small(mark,size:32,background:false))
    let text = """
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" role="img" aria-label="Kiri">
    <style>.tiny,.small{display:none}@media(max-width:23px){.detail{display:none}.tiny{display:inline}}@media(min-width:24px) and (max-width:39px){.detail{display:none}.small{display:inline}}</style>
    <path fill="#111418" d="M0 0H1024V1024H0Z"/>
    <g fill="#f0f3f5"><path class="tiny" d="\(svgPath(p16))"/><path class="small" d="\(svgPath(p32))"/><path class="detail" d="\(svgPath(mark))"/></g>
    </svg>
    """
    try write(text,url)
}

func little<T:FixedWidthInteger>(_ value:T) -> Data {
    var v = value.littleEndian
    return withUnsafeBytes(of:&v) { Data($0) }
}
// PNG frames in a standard Windows ICONDIR; each frame has an explicit size.
func ico(_ frames: [(Int,URL)], _ url: URL) throws {
    let payloads = try frames.map { try Data(contentsOf:$0.1) }
    var data = little(UInt16(0))+little(UInt16(1))+little(UInt16(frames.count))
    var offset = 6+16*frames.count
    for (i,frame) in frames.enumerated() {
        data += Data([UInt8(frame.0 == 256 ? 0 : frame.0),UInt8(frame.0 == 256 ? 0 : frame.0),0,0])
        data += little(UInt16(1))+little(UInt16(32))+little(UInt32(payloads[i].count))+little(UInt32(offset))
        offset += payloads[i].count
    }
    for payload in payloads { data += payload }
    try write(data,url)
}
func run(_ executable: String, _ arguments: [String]) throws {
    let task = Process(); task.executableURL = URL(fileURLWithPath:executable); task.arguments=arguments
    try task.run(); task.waitUntilExit()
    if task.terminationStatus != 0 { throw Failure("\(executable) failed") }
}
struct Failure: Error { let message: String; init(_ message:String) { self.message=message } }

func loadTexture(_ name:String) throws -> Texture? {
    let url=brand.appendingPathComponent(name+"/source/gunmetal-fog.png")
    guard fm.fileExists(atPath:url.path) else { return nil }
    let data=try Data(contentsOf:url)
    guard let image=NSBitmapImageRep(data:data)?.cgImage else { throw Failure("Cannot decode background") }
    return Texture(image:image,data:data)
}
func export(_ name: String) throws -> CGPath {
    let base = brand.appendingPathComponent(name)
    func file(_ path:String) -> URL { base.appendingPathComponent(path) }
    let mark = try trace(file("source/generated-stencil.png"))
    let texture = try loadTexture(name)
    for paint in [Paint.silver,.white,.black] {
        let art = Art(path:mark,width:1024,height:1024,paint:paint)
        try svg(art,file("mark/mark-\(paint.rawValue).svg"))
        try pdf(art,file("mark/mark-\(paint.rawValue).pdf"))
        for n in (paint == .silver ? [64,128,256,512,1024,2048] : [256,1024]) {
            try png(raster(art,width:n),file("mark/mark-\(paint.rawValue)-\(n).png"))
        }
        for stacked in [false,true] {
            let art = logo(mark,stacked:stacked,paint:paint)
            let label = "logo/\(stacked ? "stacked" : "horizontal")-\(paint.rawValue)"
            try svg(art,file(label+".svg")); try pdf(art,file(label+".pdf"))
            for n in [256,512,1024,2048] { try png(raster(art,width:n),file(label+"-\(n).png")) }
        }
    }
    let tileMark = transformed(mark,CGAffineTransform(a:0.88,b:0,c:0,d:0.88,tx:61.44,ty:61.44))
    let app = Art(path:tileMark,width:1024,height:1024,tile:true,texture:texture)
    try svg(app,file("desktop/app-icon.svg"))
    try svg(Art(path:tileMark,width:1024,height:1024,tile:true),file("desktop/app-icon-flat.svg"))
    for n in sizes {
        try png(n < 48 ? small(mark,size:n,background:true) : raster(app,width:n),file("desktop/png/icon-\(n).png"))
    }
    try png(raster(app,width:2048),file("desktop/png/icon-2048.png"))
    try ico([16,24,32,48,64,128,256].map { ($0,file("desktop/png/icon-\($0).png")) },file("desktop/kiri.ico"))
    let iconset = file("desktop/kiri.iconset")
    for n in [16,32,128,256,512] {
        for scale in [1,2] {
            let px = n*scale
            let image = px < 48 ? small(mark,size:px,background:true) : raster(app,width:px)
            try png(image,iconset.appendingPathComponent("icon_\(n)x\(n)\(scale == 2 ? "@2x" : "").png"))
        }
    }
    try run("/usr/bin/iconutil",["-c","icns",iconset.path,"-o",file("desktop/kiri.icns").path])
    let web = Art(path:mark,width:1024,height:1024,background:true,texture:texture)
    try favicon(mark,file("web/favicon.svg"))
    try svg(Art(path:mark,width:1024,height:1024,paint:.black),file("web/safari-pinned-tab.svg"))
    for n in [16,32,48,64,96] {
        try png(n < 48 ? small(mark,size:n,background:true) : raster(web,width:n),file("web/favicon-\(n).png"))
    }
    try ico([16,32,48].map { ($0,file("web/favicon-\($0).png")) },file("web/favicon.ico"))
    try png(raster(web,width:180),file("web/apple-touch-icon.png"))
    for n in [192,512] {
        try png(raster(web,width:n),file("web/icon-\(n).png"))
        let safe = transformed(mark,CGAffineTransform(a:0.70,b:0,c:0,d:0.70,tx:153.6,ty:153.6))
        try png(raster(Art(path:safe,width:1024,height:1024,background:true,texture:texture),width:n),file("web/maskable-\(n).png"))
        try png(raster(Art(path:CGMutablePath(),width:1024,height:1024,background:true,texture:texture),width:n),file("web/background-\(n).png"))
    }
    let manifest: [String:Any] = ["name":"Kiri","short_name":"Kiri","id":"./","start_url":"./","display":"standalone","background_color":"#111418","theme_color":"#111418","icons":[
        ["src":"icon-192.png","sizes":"192x192","type":"image/png","purpose":"any"],
        ["src":"icon-512.png","sizes":"512x512","type":"image/png","purpose":"any"],
        ["src":"maskable-192.png","sizes":"192x192","type":"image/png","purpose":"maskable"],
        ["src":"maskable-512.png","sizes":"512x512","type":"image/png","purpose":"maskable"]]]
    try write(JSONSerialization.data(withJSONObject:manifest,options:[.prettyPrinted,.sortedKeys]),file("web/site.webmanifest"))
    try write("""
    <!-- Copy this entire web folder to /brand/\(name)/, or adjust these URLs. -->
    <link rel="icon" href="/brand/\(name)/favicon.ico" sizes="16x16 32x32 48x48">
    <link rel="icon" type="image/svg+xml" href="/brand/\(name)/favicon.svg">
    <link rel="apple-touch-icon" sizes="180x180" href="/brand/\(name)/apple-touch-icon.png">
    <link rel="mask-icon" href="/brand/\(name)/safari-pinned-tab.svg" color="#111418">
    <link rel="manifest" href="/brand/\(name)/site.webmanifest">
    <meta name="theme-color" content="#111418">
    """,file("web/head.html"))
    // Vector source for the unaccompanied wordmark, shared across both designs.
    try svg(Art(path:transformed(wordmark(),CGAffineTransform(translationX:16,y:8)),width:296,height:168,paint:.black),file("logo/wordmark-black.svg"))
    try svg(Art(path:transformed(wordmark(),CGAffineTransform(translationX:16,y:8)),width:296,height:168,paint:.white),file("logo/wordmark-white.svg"))
    print("Exported \(name)")
    return mark
}

func label(_ text:String, _ ctx:CGContext, x:CGFloat, y:CGFloat, size:CGFloat, hex:UInt32=0xCDD2D8) {
    ctx.saveGState(); ctx.translateBy(x:x,y:y); ctx.scaleBy(x:1,y:-1)
    let font = CTFontCreateWithName("HelveticaNeue" as CFString,size,nil)
    let line = CTLineCreateWithAttributedString(NSAttributedString(string:text,attributes:[
        NSAttributedString.Key(kCTFontAttributeName as String):font,
        NSAttributedString.Key(kCTForegroundColorAttributeName as String):color(hex)]))
    ctx.textPosition = .zero; CTLineDraw(line,ctx); ctx.restoreGState()
}
func preview(_ marks:[CGPath]) throws {
    let w=1800, h=1420
    let ctx=CGContext(data:nil,width:w,height:h,bitsPerComponent:8,bytesPerRow:w*4,space:space,bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
    ctx.translateBy(x:0,y:CGFloat(h));ctx.scaleBy(x:1,y:-1)
    ctx.setFillColor(color(0x0D1014));ctx.fill(CGRect(x:0,y:0,width:w,height:h))
    label("kiri / production identity",ctx,x:80,y:83,size:30)
    label("Two approved directions. Vector artwork, visible pixel dithering, native icon exports.",ctx,x:80,y:121,size:18,hex:0x89939E)
    for (i,mark) in marks.enumerated() {
        let x=CGFloat(80+i*870)
        label(i == 0 ? "04  CONVERGENCE" : "05  HORIZON",ctx,x:x,y:187,size:16,hex:0x89939E)
        ctx.saveGState();ctx.translateBy(x:x+125,y:210);ctx.scaleBy(x:0.48,y:0.48)
        draw(Art(path:transformed(mark,CGAffineTransform(a:0.88,b:0,c:0,d:0.88,tx:61.44,ty:61.44)),width:1024,height:1024,tile:true,texture:try loadTexture(i == 0 ? "convergence" : "horizon")),ctx);ctx.restoreGState()
        ctx.saveGState();ctx.translateBy(x:x+25,y:704);ctx.scaleBy(x:0.62,y:0.62)
        draw(logo(mark,stacked:false,paint:.silver),ctx);ctx.restoreGState()
        label("FAVICON / NATIVE PIXELS",ctx,x:x,y:1040,size:14,hex:0x89939E)
        for (j,n) in [16,24,32,48,64].enumerated() {
            let img = n < 48 ? small(mark,size:n,background:true) : raster(Art(path:mark,width:1024,height:1024,background:true),width:n)
            let xx=x+CGFloat(j*100)
            ctx.saveGState();ctx.translateBy(x:xx,y:1080+CGFloat(n));ctx.scaleBy(x:1,y:-1)
            ctx.interpolationQuality = .none;ctx.draw(img,in:CGRect(x:0,y:0,width:n,height:n));ctx.restoreGState()
            label("\(n)",ctx,x:xx,y:1174,size:14,hex:0x89939E)
        }
        ctx.setFillColor(color(0xECEEF0));ctx.fill(CGRect(x:x,y:1210,width:740,height:126))
        ctx.saveGState();ctx.translateBy(x:x+200,y:1206);ctx.scaleBy(x:0.28,y:0.28)
        draw(logo(mark,stacked:false,paint:.black),ctx);ctx.restoreGState()
    }
    try png(ctx.makeImage()!,brand.appendingPathComponent("preview.png"))
}

do {
    let marks = try ["convergence","horizon"].map(export)
    try preview(marks)
} catch { fputs("Brand export failed: \(error)\n",stderr); exit(1) }
