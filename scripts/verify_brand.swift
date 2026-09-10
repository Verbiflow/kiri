#!/usr/bin/env swift
// Verify the delivered files rather than relying on the exporter succeeding.
import AppKit
import Foundation
import CryptoKit

struct Invalid: Error { let message: String; init(_ m:String) { message=m } }
func check(_ condition:Bool,_ message:String) throws { if !condition { throw Invalid(message) } }
let root = URL(fileURLWithPath:FileManager.default.currentDirectoryPath).appendingPathComponent("assets/brand")
let fm = FileManager.default
func bitmap(_ url:URL) throws -> NSBitmapImageRep {
    guard let b=NSBitmapImageRep(data:try Data(contentsOf:url)) else { throw Invalid("Cannot decode \(url.path)") }
    return b
}
func alpha(_ b:NSBitmapImageRep,_ x:Int,_ y:Int) -> CGFloat { b.colorAt(x:x,y:y)!.alphaComponent }
func u16(_ d:Data,_ i:Int) -> Int { Int(d[i]) | Int(d[i+1])<<8 }
func u32(_ d:Data,_ i:Int) -> Int { u16(d,i) | u16(d,i+2)<<16 }
func be32(_ d:Data,_ i:Int) -> Int { Int(d[i])<<24 | Int(d[i+1])<<16 | Int(d[i+2])<<8 | Int(d[i+3]) }
func verifyICO(_ url:URL,_ expected:[Int]) throws {
    let d=try Data(contentsOf:url)
    try check(d.count >= 6 && u16(d,0)==0 && u16(d,2)==1,"Invalid ICO header")
    let count=u16(d,4)
    try check(count==expected.count && d.count >= 6+count*16,"ICO frame count")
    for i in 0..<count {
        let p=6+i*16
        let width=d[p]==0 ? 256 : Int(d[p])
        let height=d[p+1]==0 ? 256 : Int(d[p+1])
        let length=u32(d,p+8), offset=u32(d,p+12)
        try check(width==expected[i] && height==width && offset+length<=d.count,"ICO frame metadata")
        guard let b=NSBitmapImageRep(data:d.subdata(in:offset..<(offset+length))) else { throw Invalid("ICO payload cannot decode") }
        try check(b.pixelsWide==width && b.pixelsHigh==height,"ICO frame dimensions")
    }
}
func verifyICNS(_ url:URL) throws {
    let d=try Data(contentsOf:url)
    try check(d.count>8 && String(data:d.prefix(4),encoding:.ascii)=="icns" && be32(d,4)==d.count,"ICNS header")
    var offset=8, types=Set<String>()
    while offset<d.count {
        try check(offset+8<=d.count,"ICNS truncated chunk")
        let length=be32(d,offset+4)
        try check(length>=8 && offset+length<=d.count,"ICNS chunk length")
        types.insert(String(data:d.subdata(in:offset..<(offset+4)),encoding:.ascii)!)
        offset += length
    }
    try check(types.contains("ic10"),"ICNS missing 1024px representation")
    let temp=fm.temporaryDirectory.appendingPathComponent(UUID().uuidString+".iconset")
    defer { try? fm.removeItem(at:temp) }
    let process=Process();process.executableURL=URL(fileURLWithPath:"/usr/bin/iconutil")
    process.arguments=["-c","iconset",url.path,"-o",temp.path]
    try process.run();process.waitUntilExit()
    try check(process.terminationStatus==0,"ICNS cannot unpack")
    for n in [16,32,128,256,512] { for scale in [1,2] {
        let b=try bitmap(temp.appendingPathComponent("icon_\(n)x\(n)\(scale==2 ? "@2x" : "").png"))
        try check(b.pixelsWide==n*scale && b.pixelsHigh==n*scale,"ICNS decoded size")
    } }
}
func compareRendering(_ vector:URL,_ png:URL) throws {
    guard let image=NSImage(contentsOf:vector),let cg=image.cgImage(forProposedRect:nil,context:nil,hints:nil) else { throw Invalid("Cannot render \(vector.lastPathComponent)") }
    let rendered=NSBitmapImageRep(cgImage:cg)
    let expected=try bitmap(png)
    var alphaError=0.0, colorError=0.0, colorCount=0
    for y in 0..<64 { for x in 0..<64 {
        let actual=rendered.colorAt(x:x*rendered.pixelsWide/64,y:y*rendered.pixelsHigh/64)!.usingColorSpace(.sRGB)!
        let reference=expected.colorAt(x:x*expected.pixelsWide/64,y:y*expected.pixelsHigh/64)!.usingColorSpace(.sRGB)!
        alphaError += Double(abs(actual.alphaComponent-reference.alphaComponent))
        if actual.alphaComponent>0.98 && reference.alphaComponent>0.98 {
            colorError += Double(abs(actual.redComponent-reference.redComponent));colorCount += 1
        }
    } }
    try check(alphaError/4096 < 0.035,"Vector/PNG silhouette mismatch: \(vector.lastPathComponent), \(alphaError/4096)")
    try check(colorCount>0 && colorError/Double(colorCount) < 0.045,"Vector/PNG tone mismatch: \(vector.lastPathComponent), \(colorError/Double(max(1,colorCount)))")
}
// Independent of the exporter's raw byte access: Cocoa decodes source colors.
// This catches a consistently broken trace shared by every exported format.
func compareSource(_ sourceURL:URL,_ outputURL:URL) throws {
    let source=try bitmap(sourceURL), output=try bitmap(outputURL)
    var components=[Int](repeating:0,count:source.samplesPerPixel)
    func foreground(_ x:Int,_ y:Int) -> Bool {
        guard x>=0,y>=0,x<source.pixelsWide,y<source.pixelsHigh else { return false }
        source.getPixel(&components,atX:x,y:y)
        return min(components[0],components[1],components[2])>204
    }
    var left=source.pixelsWide,top=source.pixelsHigh,right=0,bottom=0
    for y in 0..<source.pixelsHigh {
        for x in 0..<source.pixelsWide where foreground(x,y) {
            left=min(left,x);right=max(right,x+1);top=min(top,y);bottom=max(bottom,y+1)
        }
    }
    try check(right>left && bottom>top,"Empty source artwork")
    let scale=800.0/Double(max(right-left,bottom-top))
    let cx=Double(left+right)/2,cy=Double(top+bottom)/2
    var intersection=0,union=0
    for y in stride(from:0,to:output.pixelsHigh,by:4) { for x in stride(from:0,to:output.pixelsWide,by:4) {
        let sx=Int(floor(((Double(x)+0.5)*1024/Double(output.pixelsWide)-512)/scale+cx))
        let sy=Int(floor(((Double(y)+0.5)*1024/Double(output.pixelsHigh)-512)/scale+cy))
        let expected=foreground(sx,sy), actual=alpha(output,x,y)>0.5
        if expected && actual { intersection += 1 }
        if expected || actual { union += 1 }
    } }
    let overlap=Double(intersection)/Double(max(1,union))
    try check(overlap>0.94,"Source artwork mismatch: \(sourceURL.deletingLastPathComponent().deletingLastPathComponent().lastPathComponent), silhouette overlap \(overlap)")
    print("Source silhouette overlap: \(String(format:"%.2f",overlap*100))%")
}
do {
    var report:[String:Any]=[:]
    for name in ["convergence","horizon"] {
        let base=root.appendingPathComponent(name)
        try compareSource(base.appendingPathComponent("source/generated-stencil.png"),base.appendingPathComponent("mark/mark-white-1024.png"))
        let files=(fm.enumerator(at:base,includingPropertiesForKeys:[.isRegularFileKey])!.allObjects as! [URL]).filter {
            (try? $0.resourceValues(forKeys:[.isRegularFileKey]).isRegularFile)==true && $0.lastPathComponent != "checksums.sha256"
        }.sorted { $0.path < $1.path }
        var counts:[String:Int]=[:], hashes:[String:String]=[:]
        for url in files {
            let relative=String(url.path.dropFirst(base.path.count+1))
            counts[url.pathExtension,default:0] += 1
            let data=try Data(contentsOf:url)
            hashes[relative]=SHA256.hash(data:data).map { String(format:"%02x",$0) }.joined()
            if url.pathExtension=="png" && !relative.hasPrefix("source/") {
                let b=try bitmap(url)
                try check(b.pixelsWide>0 && b.pixelsHigh>0,"Empty PNG")
                if let regex=try? NSRegularExpression(pattern:"-(\\d+)\\.png$"),
                   let m=regex.firstMatch(in:relative,range:NSRange(relative.startIndex...,in:relative)),
                   let range=Range(m.range(at:1),in:relative),let expected=Int(relative[range]) {
                    try check(b.pixelsWide==expected,"Wrong PNG width: \(relative)")
                    let height=relative.contains("horizontal-") ? Int((Double(expected)*512/1152).rounded()) : expected
                    try check(b.pixelsHigh==height,"Wrong PNG height: \(relative)")
                }
                if relative.hasPrefix("mark/") || relative.hasPrefix("logo/") {
                    try check(alpha(b,0,0)==0 && alpha(b,b.pixelsWide-1,b.pixelsHigh-1)==0,"Baked background: \(relative)")
                }
                if relative.hasPrefix("web/") {
                    for (x,y) in [(0,0),(b.pixelsWide/2,b.pixelsHigh/2),(b.pixelsWide-1,b.pixelsHigh-1)] {
                        try check(alpha(b,x,y)==1,"Web icon must be opaque: \(relative)")
                    }
                }
            }
            if url.pathExtension=="svg" {
                let xml=String(decoding:data,as:UTF8.self)
                let texturedApp = name == "horizon" && relative == "desktop/app-icon.svg"
                if !texturedApp { try check(!xml.contains("<image") && !xml.contains("data:image"),"SVG is not outlined vector") }
                try check(!xml.contains("<text"),"SVG contains a font dependency")
                try check(XMLParser(data:data).parse(),"Malformed SVG: \(relative)")
            }
            if url.pathExtension=="pdf" {
                guard let pdf=CGPDFDocument(url as CFURL),let page=pdf.page(at:1) else { throw Invalid("Unreadable PDF") }
                try check(pdf.numberOfPages==1 && page.getBoxRect(.mediaBox).width>0,"Invalid PDF page")
            }
        }
        try verifyICO(base.appendingPathComponent("desktop/kiri.ico"),[16,24,32,48,64,128,256])
        try verifyICO(base.appendingPathComponent("web/favicon.ico"),[16,32,48])
        try verifyICNS(base.appendingPathComponent("desktop/kiri.icns"))
        for ext in ["svg","pdf"] {
            try compareRendering(base.appendingPathComponent("mark/mark-silver.\(ext)"),base.appendingPathComponent("mark/mark-silver-2048.png"))
            try compareRendering(base.appendingPathComponent("logo/horizontal-silver.\(ext)"),base.appendingPathComponent("logo/horizontal-silver-2048.png"))
        }
        try compareRendering(base.appendingPathComponent("desktop/app-icon.svg"),base.appendingPathComponent("desktop/png/icon-2048.png"))
        let manifest=try JSONSerialization.jsonObject(with:Data(contentsOf:base.appendingPathComponent("web/site.webmanifest"))) as! [String:Any]
        for icon in manifest["icons"] as! [[String:String]] {
            let b=try bitmap(base.appendingPathComponent("web/"+icon["src"]!))
            try check(icon["sizes"]=="\(b.pixelsWide)x\(b.pixelsHigh)","Manifest size mismatch")
            if icon["purpose"]=="maskable" {
                let background=try bitmap(base.appendingPathComponent("web/background-\(b.pixelsWide).png"))
                for y in 0..<b.pixelsHigh { for x in 0..<b.pixelsWide {
                    let c=b.colorAt(x:x,y:y)!.usingColorSpace(.deviceRGB)!
                    let bg=background.colorAt(x:x,y:y)!.usingColorSpace(.deviceRGB)!
                    if abs(c.redComponent-bg.redComponent) > 0.03 {
                        let dx=(Double(x)+0.5)/Double(b.pixelsWide)-0.5
                        let dy=(Double(y)+0.5)/Double(b.pixelsHigh)-0.5
                        try check(dx*dx+dy*dy <= 0.16,"Maskable art exceeds central 80% circle: \(name) \(icon["src"]!) at \(x),\(y), foreground \(c.redComponent), background \(bg.redComponent)")
                    }
                } }
            }
        }
        report[name]=["status":"passed","file_count":files.count,"formats":counts,"sha256":hashes]
        let checksums=hashes.keys.sorted().map { hashes[$0]!+"  "+$0 }.joined(separator:"\n")+"\n"
        try checksums.write(to:base.appendingPathComponent("checksums.sha256"),atomically:true,encoding:.utf8)
        print("\(name): \(files.count) files verified; PNG dimensions/alpha, SVG/PDF rendering, ICO frames, ICNS round-trip, manifest, maskable safe area passed")
    }
    try JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]).write(to:root.appendingPathComponent("verification.json"),options:.atomic)
} catch { fputs("Brand verification failed: \(error)\n",stderr);exit(1) }
