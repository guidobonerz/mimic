"""Shared part of the mimic user guide builder: fonts, styles, block helpers,
the title page with the logo and the page template. The text lives in
guide_en.py and guide_de.py; tools/user_guide.py builds both PDFs. Needs
reportlab (pip install reportlab).
"""

import os
import re
import sys
from datetime import date
from xml.sax.saxutils import escape

from reportlab.lib import colors
from reportlab.lib.enums import TA_CENTER
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import cm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import (
    BaseDocTemplate,
    CondPageBreak,
    Frame,
    Image,
    KeepTogether,
    PageBreak,
    PageTemplate,
    Paragraph,
    Preformatted,
    Spacer,
    Table,
    TableStyle,
)
from reportlab.platypus.tableofcontents import TableOfContents

# ---------------------------------------------------------------- fonts --

FONT_DIRS = [
    r"C:\Windows\Fonts",
    "/usr/share/fonts/truetype/dejavu",
    "/Library/Fonts",
]


def register_fonts():
    """Segoe UI + Consolas on Windows, DejaVu elsewhere (both have the
    arrows and the pound sign the key tables use)."""
    candidates = [
        ("segoeui.ttf", "segoeuib.ttf", "segoeuii.ttf", "consola.ttf", "consolab.ttf"),
        ("DejaVuSans.ttf", "DejaVuSans-Bold.ttf", "DejaVuSans-Oblique.ttf",
         "DejaVuSansMono.ttf", "DejaVuSansMono-Bold.ttf"),
    ]
    for d in FONT_DIRS:
        for names in candidates:
            paths = [os.path.join(d, n) for n in names]
            if all(os.path.exists(p) for p in paths):
                for name, path in zip(["Sans", "Sans-Bold", "Sans-Italic", "Mono", "Mono-Bold"], paths):
                    pdfmetrics.registerFont(TTFont(name, path))
                pdfmetrics.registerFontFamily(
                    "Sans", normal="Sans", bold="Sans-Bold", italic="Sans-Italic",
                    boldItalic="Sans-Bold")
                pdfmetrics.registerFontFamily(
                    "Mono", normal="Mono", bold="Mono-Bold", italic="Mono",
                    boldItalic="Mono-Bold")
                return
    sys.exit("no suitable TrueType fonts found (Segoe UI/Consolas or DejaVu)")


register_fonts()

# --------------------------------------------------------------- styles --

INK = colors.HexColor("#1d2330")
ACCENT = colors.HexColor("#2f5d9e")
MUTED = colors.HexColor("#5b6475")
RULE = colors.HexColor("#c9d1de")
CODE_BG = colors.HexColor("#f2f4f8")
HEAD_BG = colors.HexColor("#e4eaf3")

BASE = ParagraphStyle("base", fontName="Sans", fontSize=9.6, leading=13.4,
                      textColor=INK, spaceAfter=5)
STYLES = {
    "title": ParagraphStyle("title", parent=BASE, fontName="Sans-Bold", fontSize=30,
                            leading=36, textColor=ACCENT, alignment=TA_CENTER),
    "subtitle": ParagraphStyle("subtitle", parent=BASE, fontSize=13, leading=18,
                               textColor=MUTED, alignment=TA_CENTER),
    "h1": ParagraphStyle("h1", parent=BASE, fontName="Sans-Bold", fontSize=17,
                         leading=22, textColor=ACCENT, spaceBefore=4, spaceAfter=8),
    "h2": ParagraphStyle("h2", parent=BASE, fontName="Sans-Bold", fontSize=12.5,
                         leading=16, textColor=INK, spaceBefore=10, spaceAfter=5),
    "h3": ParagraphStyle("h3", parent=BASE, fontName="Sans-Bold", fontSize=10.5,
                         leading=14, textColor=INK, spaceBefore=7, spaceAfter=3),
    "body": BASE,
    "bullet": ParagraphStyle("bullet", parent=BASE, leftIndent=14, bulletIndent=4,
                             spaceAfter=2.5),
    "cell": ParagraphStyle("cell", parent=BASE, fontSize=8.6, leading=11.2, spaceAfter=0),
    "cellhead": ParagraphStyle("cellhead", parent=BASE, fontName="Sans-Bold", fontSize=8.6,
                               leading=11.2, spaceAfter=0),
    "code": ParagraphStyle("code", fontName="Mono", fontSize=8.2, leading=10.6,
                           textColor=INK),
    "note": ParagraphStyle("note", parent=BASE, fontSize=9, leading=12.6,
                           textColor=INK, leftIndent=8, rightIndent=8),
    "toc1": ParagraphStyle("toc1", parent=BASE, fontName="Sans-Bold", fontSize=10.5,
                           leading=15, leftIndent=0),
    "toc2": ParagraphStyle("toc2", parent=BASE, fontSize=9.4, leading=12.5,
                           leftIndent=16, textColor=MUTED),
}

# ---------------------------------------------------------- inline text --

INLINE = re.compile(r"`([^`]+)`|\*\*([^*]+)\*\*|\*([^*]+)\*")


def rich(text):
    """`code`, **bold** and *italic* into reportlab markup; the rest escaped."""
    out, pos = [], 0
    for m in INLINE.finditer(text):
        out.append(escape(text[pos:m.start()]))
        code, bold, ital = m.groups()
        if code is not None:
            out.append('<font face="Mono" color="#24406b">%s</font>' % escape(code))
        elif bold is not None:
            out.append("<b>%s</b>" % escape(bold))
        else:
            out.append("<i>%s</i>" % escape(ital))
        pos = m.end()
    out.append(escape(text[pos:]))
    return "".join(out)


# --------------------------------------------------------------- blocks --

story = []
_counters = [0, 0]


def h1(text):
    _counters[0] += 1
    _counters[1] = 0
    story.append(PageBreak())
    p = Paragraph("%d&nbsp;&nbsp;%s" % (_counters[0], escape(text)), STYLES["h1"])
    p.toc_level = 0
    story.append(p)


def h2(text):
    _counters[1] += 1
    story.append(CondPageBreak(3 * cm))
    p = Paragraph("%d.%d&nbsp;&nbsp;%s" % (_counters[0], _counters[1], escape(text)),
                  STYLES["h2"])
    p.toc_level = 1
    story.append(p)


def h3(text):
    story.append(CondPageBreak(2.2 * cm))
    story.append(Paragraph(rich(text), STYLES["h3"]))


def p(text):
    story.append(Paragraph(rich(" ".join(text.split())), STYLES["body"]))


def bullets(*items):
    for it in items:
        story.append(Paragraph(rich(" ".join(it.split())), STYLES["bullet"], bulletText="•"))
    story.append(Spacer(1, 3))


def code(text):
    text = text.strip("\n")
    pre = Preformatted(text, STYLES["code"])
    t = Table([[pre]], colWidths=[None])
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), CODE_BG),
        ("BOX", (0, 0), (-1, -1), 0.5, RULE),
        ("LEFTPADDING", (0, 0), (-1, -1), 7),
        ("RIGHTPADDING", (0, 0), (-1, -1), 7),
        ("TOPPADDING", (0, 0), (-1, -1), 5),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 5),
    ]))
    story.append(t)
    story.append(Spacer(1, 6))


def note(text, label="Note"):
    para = Paragraph("<b>%s.</b> %s" % (label, rich(" ".join(text.split()))), STYLES["note"])
    t = Table([[para]], colWidths=[None])
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, -1), colors.HexColor("#eef3fb")),
        ("LINEBEFORE", (0, 0), (0, -1), 2.5, ACCENT),
        ("LEFTPADDING", (0, 0), (-1, -1), 6),
        ("TOPPADDING", (0, 0), (-1, -1), 5),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 5),
    ]))
    story.append(t)
    story.append(Spacer(1, 7))


def table(head, rows, widths):
    """`widths` in fractions of the text width."""
    total = A4[0] - 4.2 * cm
    data = [[Paragraph(rich(h), STYLES["cellhead"]) for h in head]]
    for r in rows:
        data.append([Paragraph(rich(" ".join(str(c).split())), STYLES["cell"]) for c in r])
    t = Table(data, colWidths=[w * total for w in widths], repeatRows=1)
    t.setStyle(TableStyle([
        ("BACKGROUND", (0, 0), (-1, 0), HEAD_BG),
        ("LINEBELOW", (0, 0), (-1, 0), 0.8, ACCENT),
        ("LINEBELOW", (0, 1), (-1, -1), 0.3, RULE),
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("LEFTPADDING", (0, 0), (-1, -1), 4),
        ("RIGHTPADDING", (0, 0), (-1, -1), 4),
        ("TOPPADDING", (0, 0), (-1, -1), 3),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 3),
    ]))
    story.append(t)
    story.append(Spacer(1, 8))



# ------------------------------------------------------------ languages --

LOGO = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                    "Futuristic MIMIC Tech Emblem.png")


def logo_aspect():
    """Height over width of the logo file (transparent PNG, any size)."""
    from reportlab.lib.utils import ImageReader
    w, h = ImageReader(LOGO).getSize()
    return h / w

MONTHS_DE = ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August",
             "September", "Oktober", "November", "Dezember"]

TEXT = {
    "en": {
        "title": "User Guide",
        "tagline": "Imitates Machines, Inspects Code",
        "blurb": "Running the emulator: desktop, command line, machines, media, live "
                 "preview and debugger — and the complete reference of every option, "
                 "the start profiles and the web front end",
        "contents": "Contents",
        "header": "mimic — User Guide",
        "date": lambda d: d.strftime("%d %B %Y"),
    },
    "de": {
        "title": "Benutzerhandbuch",
        "tagline": "Imitates Machines, Inspects Code",
        "blurb": "Der Emulator in der Anwendung: Desktop, Kommandozeile, Maschinen, Medien, "
                 "Live-Vorschau und Debugger — und die vollständige Referenz aller Optionen, "
                 "der Startprofile und der Web-Oberfläche",
        "contents": "Inhalt",
        "header": "mimic — Benutzerhandbuch",
        "date": lambda d: "%d. %s %d" % (d.day, MONTHS_DE[d.month - 1], d.year),
    },
}

PART_STYLE = ParagraphStyle("part", parent=STYLES["title"], fontSize=24, leading=30,
                            textColor=ACCENT, alignment=TA_CENTER, spaceBefore=6 * cm)
PART_SUB = ParagraphStyle("partsub", parent=STYLES["subtitle"], spaceBefore=0.6 * cm)


def part(title, blurb):
    """A part title on a page of its own (listed in the contents, unnumbered)."""
    story.append(PageBreak())
    para = Paragraph(escape(title), PART_STYLE)
    para.toc_level = 0
    story.append(para)
    story.append(Paragraph(escape(blurb), PART_SUB))


def reset():
    story.clear()
    _counters[:] = [0, 0]


def front_matter(lang):
    t = TEXT[lang]
    if os.path.exists(LOGO):
        story.append(Spacer(1, 3.5 * cm))
        story.append(Image(LOGO, width=14 * cm, height=14 * cm * logo_aspect(), mask="auto"))
        story.append(Spacer(1, 1.2 * cm))
    else:
        story.append(Spacer(1, 6 * cm))
        story.append(Paragraph("mimic", STYLES["title"]))
        story.append(Spacer(1, 0.4 * cm))
    story.append(Paragraph(t["tagline"], STYLES["subtitle"]))
    story.append(Spacer(1, 1.2 * cm))
    story.append(Paragraph(t["title"], ParagraphStyle(
        "ug", parent=STYLES["subtitle"], fontName="Sans-Bold", fontSize=18, leading=24,
        textColor=INK)))
    story.append(Spacer(1, 0.3 * cm))
    story.append(Paragraph(t["blurb"], STYLES["subtitle"]))
    story.append(Spacer(1, 4 * cm))
    story.append(Paragraph(t["date"](date.today()), STYLES["subtitle"]))
    story.append(PageBreak())
    toc = TableOfContents()
    toc.levelStyles = [STYLES["toc1"], STYLES["toc2"]]
    story.append(Paragraph(t["contents"], STYLES["h1"]))
    story.append(toc)


class Doc(BaseDocTemplate):
    def __init__(self, path, lang):
        self.header = TEXT[lang]["header"]
        super().__init__(path, pagesize=A4, leftMargin=2.1 * cm, rightMargin=2.1 * cm,
                         topMargin=2.0 * cm, bottomMargin=2.0 * cm,
                         title="mimic " + TEXT[lang]["title"], author="mimic",
                         subject=TEXT[lang]["blurb"], lang=lang)
        frame = Frame(self.leftMargin, self.bottomMargin, self.width, self.height, id="f")
        self.addPageTemplates([PageTemplate(id="page", frames=[frame], onPage=self.decorate)])

    def decorate(self, canvas, doc):
        if doc.page == 1:
            return
        canvas.saveState()
        canvas.setStrokeColor(RULE)
        canvas.setLineWidth(0.5)
        y = A4[1] - 1.4 * cm
        canvas.line(self.leftMargin, y, A4[0] - self.rightMargin, y)
        canvas.setFont("Sans", 8)
        canvas.setFillColor(MUTED)
        canvas.drawString(self.leftMargin, y + 4, self.header)
        if os.path.exists(LOGO):
            w = 2.2 * cm
            canvas.drawImage(LOGO, A4[0] - self.rightMargin - w, y + 3, width=w,
                             height=w * logo_aspect(), mask="auto", preserveAspectRatio=True)
        canvas.drawRightString(A4[0] - self.rightMargin, 1.2 * cm, str(doc.page))
        canvas.restoreState()

    def afterFlowable(self, flowable):
        level = getattr(flowable, "toc_level", None)
        if level is not None:
            self.notify("TOCEntry", (level, flowable.getPlainText(), self.page))
