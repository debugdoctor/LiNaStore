#!/usr/bin/env python3
"""Generate docs/architecture.svg — detailed LiNaStore architecture.

Shows the request pipeline (admission -> meta queue + payload channel -> worker),
PUT/GET data direction, in-flight/active limits, io-wait parking, and the
streaming/backpressure behavior (payload channel full -> send().await -> client
is paused via TCP). SVG is hand-written with the stdlib only.
"""
from pathlib import Path

W, H = 1240, 560
BLUE = "#2563eb"    # meta / control path
TEAL = "#0d9488"    # PUT body data path
ORANGE = "#ea580c"  # GET response data path
TEXT = "#111"


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def box(x, y, w, h, title, lines, fill, stroke, title_size=13, body_size=10.5):
    parts = [f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="8" fill="{fill}" stroke="{stroke}" stroke-width="2"/>']
    ty = y + 18
    parts.append(f'<text x="{x+w/2}" y="{ty}" text-anchor="middle" font-family="monospace" '
                 f'font-size="{title_size}" font-weight="bold" fill="{TEXT}">{esc(title)}</text>')
    for i, line in enumerate(lines):
        parts.append(f'<text x="{x+w/2}" y="{ty+15+i*13}" text-anchor="middle" font-family="monospace" '
                     f'font-size="{body_size}" fill="#333">{esc(line)}</text>')
    return "".join(parts)


def label(mx, my, text, color, anchor="middle", size=10.5):
    return (f'<text x="{mx}" y="{my}" text-anchor="{anchor}" font-family="monospace" '
            f'font-size="{size}" fill="{color}">{esc(text)}</text>')


def arrow(x1, y1, x2, y2, color=BLUE, dashed=False, both=False, width=2, lbl=None, lbl_pos=None, lbl_anchor="middle"):
    marker = 'url(#b)'
    dash = ' stroke-dasharray="6,4"' if dashed else ""
    start = f' marker-start="{marker}"' if both else ""
    out = f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{color}" stroke-width="{width}"{dash}{start} marker-end="{marker}"/>'
    if lbl:
        px, py = lbl_pos or ((x1+x2)/2, (y1+y2)/2)
        out += label(px, py, lbl, color, lbl_anchor)
    return out


def elbow(pts, color=BLUE, dashed=False, width=2, lbl=None, lbl_pos=(0, 0), lbl_anchor="middle"):
    marker = 'url(#b)'
    dash = ' stroke-dasharray="6,4"' if dashed else ""
    coords = " ".join(f"{p[0]},{p[1]}" for p in pts)
    out = f'<polyline points="{coords}" fill="none" stroke="{color}" stroke-width="{width}"{dash} marker-end="{marker}"/>'
    if lbl:
        out += label(lbl_pos[0], lbl_pos[1], lbl, color, lbl_anchor)
    return out


def main():
    body = []
    body.append(f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" font-family="monospace">')
    body.append(f'<rect x="0" y="0" width="{W}" height="{H}" fill="#fafafa"/>')
    body.append(
        '<defs><marker id="b" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" '
        'orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="#444"/></marker></defs>'
    )

    # ---- Boxes (request pipeline) ----
    body.append(box(30, 90, 240, 150, "Client / Frontend", ["HTTP / S3 / Advanced", "parse request headers",
                                                            "await admission permit", "read + push body chunks",
                                                            "write response stream"], "#dbeafe", BLUE))
    body.append(box(30, 270, 240, 100, "Admission (in-flight)", ["semaphore: full -> await", "30s timeout -> 503",
                                                                 "body not yet read"], "#fee2e2", "#dc2626", body_size=10.5))
    body.append(box(320, 90, 230, 60, "order_queue (meta)", ["intent only", "cap 128"], "#fef3c7", "#d97706", body_size=10))
    body.append(box(320, 180, 230, 70, "payload channel (data)", ["bounded mpsc", "depth = 64 \u00d7 chunk",
                                                                  "send().await backpressure"], "#fef3c7", "#d97706", body_size=10))
    body.append(box(610, 90, 260, 160, "Worker (porter task)", ["1 request = 1 task", "active = \u230a cpus/2 \u230b",
                                                                "real work only holds slot", "stream payload -> storage",
                                                                "stream response"], "#e0e7ff", "#6366f1", body_size=10.5))
    body.append(box(910, 90, 120, 160, "io wait", ["channel / DB / socket", "\u2192 await park"], "#fce7f3", "#db2777",
                    body_size=10.5))

    # ---- Storage / SQL (worker upstream) ----
    body.append(box(320, 290, 230, 160, "storage (file IO)", ["PUT: mem \u2264 4MB else spool", "incremental BLAKE3",
                                                               "dedup \u2192 link \u2192 DB", "GET: open_read stream"],
                    "#ecfdf5", TEAL, body_size=10))
    body.append(box(610, 290, 260, 160, "sql queue", ["DbExecutor", "single thread", "serial metadata ops"],
                    "#e2e8f0", "#475569", body_size=10.5))

    # ---- Frontend -> admission (vertical, bidirectional) ----
    body.append(arrow(150, 240, 150, 270, BLUE, both=True, width=3, lbl_pos=(160, 258), lbl_anchor="start",
                      lbl="await permit"))

    # ---- Frontend -> channels (horizontal) ----
    body.append(arrow(270, 120, 320, 120, BLUE, width=3, lbl_pos=(295, 108), lbl="meta"))
    body.append(arrow(270, 215, 320, 215, TEAL, width=3, lbl_pos=(295, 203), lbl="body chunks"))

    # ---- Channels -> worker ----
    body.append(arrow(550, 120, 610, 120, BLUE, dashed=True, width=3, lbl_pos=(580, 108), lbl="dequeue"))
    body.append(arrow(550, 215, 610, 215, TEAL, dashed=True, width=3, lbl_pos=(580, 203), lbl="pull"))

    # ---- Worker <-> io wait ----
    body.append(arrow(870, 170, 910, 170, BLUE, dashed=True, both=True, lbl_pos=(890, 158), lbl="park | resume"))

    # ---- Worker <-> storage / sql (diagonal, bidirectional) ----
    body.append(arrow(700, 250, 435, 290, TEAL, both=True, width=3, lbl_pos=(570, 276), lbl="file IO"))
    body.append(arrow(730, 250, 750, 290, BLUE, both=True, lbl_pos=(770, 276), lbl="metadata"))

    # ---- Response: worker -> frontend (over the top) ----
    body.append(elbow([(800, 90), (800, 40), (150, 40), (150, 90)], ORANGE, width=3,
                      lbl="response stream (GET data / PUT ack)", lbl_pos=(500, 32)))

    # ---- Backpressure annotation on the payload path ----
    body.append(label(435, 475, "PUT after admission: payload channel full -> send().await ->", TEAL, "middle", 10.5))
    body.append(label(435, 492, "frontend stops reading socket -> TCP backpressure pauses client", TEAL, "middle", 10.5))
    body.append(label(435, 509, "waiting requests never start their body; only 30s admission timeout -> 503", "#dc2626", "middle", 10.5))

    # ---- Legend ----
    body.append(label(1040, 500, "legend:", TEXT, "start", 10.5))
    body.append(label(1040, 516, "blue = meta/control   teal = PUT body   orange = GET response", TEXT, "start", 10.5))
    body.append(label(1040, 532, "dashed = backpressure / io-wait", TEXT, "start", 10.5))

    body.append("</svg>")
    Path(__file__).resolve().parent.joinpath("architecture.svg").write_text("\n".join(body))
    print("wrote docs/architecture.svg")


if __name__ == "__main__":
    main()
