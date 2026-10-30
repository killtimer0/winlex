[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing,System.Xml -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Globalization;
using System.Text.RegularExpressions;
using System.Xml;

public static class CodiconMetafile {
    public static void Convert(string input, string output) {
        var svg = new XmlDocument();
        svg.Load(input);
        var viewBox = svg.DocumentElement.GetAttribute("viewBox").Split(' ');
        float x = float.Parse(viewBox[0], CultureInfo.InvariantCulture);
        float y = float.Parse(viewBox[1], CultureInfo.InvariantCulture);
        float width = float.Parse(viewBox[2], CultureInfo.InvariantCulture);
        float height = float.Parse(viewBox[3], CultureInfo.InvariantCulture);
        float scale = (16 * 22f / 24) / Math.Max(width, height);
        using (var reference = Graphics.FromHwnd(IntPtr.Zero)) {
            var dc = reference.GetHdc();
            try {
                using (var file = new Metafile(output, dc, new RectangleF(0, 0, 16, 16),
                    MetafileFrameUnit.Pixel, EmfType.EmfPlusOnly))
                using (var graphics = Graphics.FromImage(file)) {
                    graphics.PageUnit = GraphicsUnit.Pixel;
                    graphics.SmoothingMode = SmoothingMode.AntiAlias;
                    graphics.TranslateTransform((16 - width * scale) / 2, (16 - height * scale) / 2);
                    graphics.ScaleTransform(scale, scale);
                    graphics.TranslateTransform(-x, -y);
                    foreach (XmlElement element in svg.GetElementsByTagName("path")) {
                        using (var path = Parse(element.GetAttribute("d"))) {
                            path.FillMode = element.GetAttribute("fill-rule") == "evenodd"
                                ? FillMode.Alternate : FillMode.Winding;
                            graphics.FillPath(Brushes.White, path);
                        }
                    }
                }
            } finally {
                reference.ReleaseHdc(dc);
            }
        }
    }

    static GraphicsPath Parse(string data) {
        var tokens = Regex.Matches(data, @"[A-Za-z]|[-+]?(?:\d*\.\d+|\d+\.?\d*)(?:[eE][-+]?\d+)?");
        var path = new GraphicsPath();
        int i = 0;
        char command = '\0';
        PointF current = new PointF(), start = new PointF();
        Func<float> number = () => float.Parse(tokens[i++].Value, CultureInfo.InvariantCulture);
        while (i < tokens.Count) {
            if (char.IsLetter(tokens[i].Value[0])) command = tokens[i++].Value[0];
            switch (command) {
                case 'M':
                    current = new PointF(number(), number());
                    start = current;
                    path.StartFigure();
                    command = 'L';
                    break;
                case 'L': {
                    var next = new PointF(number(), number());
                    path.AddLine(current, next);
                    current = next;
                    break;
                }
                case 'H': {
                    var next = new PointF(number(), current.Y);
                    path.AddLine(current, next);
                    current = next;
                    break;
                }
                case 'V': {
                    var next = new PointF(current.X, number());
                    path.AddLine(current, next);
                    current = next;
                    break;
                }
                case 'C': {
                    var first = new PointF(number(), number());
                    var second = new PointF(number(), number());
                    var next = new PointF(number(), number());
                    path.AddBezier(current, first, second, next);
                    current = next;
                    break;
                }
                case 'Z':
                    path.CloseFigure();
                    current = start;
                    command = '\0';
                    break;
                default:
                    path.Dispose();
                    throw new NotSupportedException("Unsupported SVG command: " + command);
            }
        }
        return path;
    }
}
'@
$directory = Join-Path (Split-Path $PSScriptRoot -Parent) 'crates/winlex-gui/res/codicons'
Get-ChildItem $directory -Filter '*.svg' | ForEach-Object {
    [CodiconMetafile]::Convert($_.FullName, [IO.Path]::ChangeExtension($_.FullName, '.emf'))
}
