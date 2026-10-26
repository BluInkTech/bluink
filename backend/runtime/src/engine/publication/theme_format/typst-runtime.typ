// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
// Private serializer support; theme packages cannot replace this code.

#let publication-style(role) = publication-theme.styles.at(role)
#let publication-align(value) = if value == "center" { center } else if value == "end" { end } else { start }
#let publication-fill(style) = if style.background == none { none } else { rgb(style.background) }
#let publication-stroke(style) = if style.borderWidthPt == 0 { none } else {
  (paint: rgb(style.borderColor), thickness: style.borderWidthPt * 1pt,
   dash: if style.borderStyle == "solid" { "solid" } else if style.borderStyle == "dotted" { "dotted" } else { "dashed" })
}

#let publication-text(role, body) = {
  let style = publication-style(role)
  set text(font: style.fontFamily, size: style.fontSizePt * 1pt,
    weight: if style.fontWeight == "bold" { 700 } else { 400 },
    style: if style.italic { "italic" } else { "normal" }, fill: rgb(style.color),
    top-edge: 0.8em, bottom-edge: -0.2em)
  body
}

#let publication-frame(style, body) = {
  let decorated = block(width: 100%, above: 0pt, below: 0pt,
    inset: style.paddingPt * 1pt, fill: publication-fill(style), stroke: publication-stroke(style),
    breakable: not style.keepTogether, body)
  block(width: 100%, above: style.spaceBeforePt * 1pt, below: style.spaceAfterPt * 1pt,
    sticky: style.keepWithNext, breakable: not style.keepTogether, pad(left: style.indentPt * 1pt, decorated))
}

#let publication-anchor(id) = if id != none { [#metadata(none)#label(id)] }

#let publication-block(role, anchor: none, body) = {
  let style = publication-style(role)
  set par(leading: (style.lineHeight - 1) * style.fontSizePt * 1pt, spacing: 0pt, justify: style.alignment == "justify")
  set align(publication-align(style.alignment))
  publication-frame(style, publication-text(role, [#publication-anchor(anchor)#body]))
}

// Paragraphs inherit the surrounding role's font, including rich notes and table headers.
#let publication-par(anchor: none, body) = {
  let style = publication-style("body")
  publication-frame(style, par([#publication-anchor(anchor)#body]))
}

#let publication-inline(role, body) = {
  let style = publication-style(role)
  box(inset: style.paddingPt * 1pt, fill: publication-fill(style), stroke: publication-stroke(style), publication-text(role, body))
}

#let publication-cell-body(role, body) = {
  let style = publication-style(role)
  set par(leading: (style.lineHeight - 1) * style.fontSizePt * 1pt, spacing: 0pt, justify: style.alignment == "justify")
  pad(left: style.indentPt * 1pt, publication-text(role, body))
}

#let publication-cell(column: 0, row: 0, colspan: 1, rowspan: 1, header: false, body) = {
  let role = if header { "tableHeader" } else { "tableCell" }
  let style = publication-style(role)
  let cell = table.cell(x: column, y: row, colspan: colspan, rowspan: rowspan, fill: publication-fill(style),
    inset: style.paddingPt * 1pt, stroke: publication-stroke(style),
    align: publication-align(style.alignment), breakable: not style.keepTogether,
    publication-cell-body(role, body))
  if header { pdf.header-cell(scope: "column", cell) } else { cell }
}

#let publication-figure(kind: image, caption: none, anchor: none, body) = {
  set figure.caption(position: if publication-style("caption").captionPosition == "before" { top } else { bottom })
  figure(kind: kind, caption: caption, supplement: none, numbering: none, [#publication-anchor(anchor)#body])
}

#let publication-rule(anchor: none) = {
  let style = publication-style("rule")
  block(above: style.spaceBeforePt * 1pt, below: style.spaceAfterPt * 1pt,
    [#publication-anchor(anchor)#line(length: 100%, stroke: publication-stroke(style))])
}

#let publication-code(code, language: "plain", anchor: none) = publication-block("code", anchor: anchor, raw(code, block: true, lang: language))
#let publication-image(source, alt, width: auto, height: auto, anchor: none) = block(width: 100%, breakable: false)[#publication-anchor(anchor)#align(center)[#image(source, alt: alt, width: width, height: height)]]

#let publication-chapter(title, first: false, body) = {
  if not first and publication-theme.page.chapterStart == "next-page" { pagebreak(weak: true) }
  metadata((kind: "bluink-chapter", title: title))
  body
}

#let publication-chapter-title() = context {
  let current = here().page()
  let chapters = query(metadata).filter(it => type(it.value) == dictionary and it.value.at("kind", default: none) == "bluink-chapter" and it.location().page() <= current)
  if chapters.len() > 0 { chapters.last().value.title }
}

#let publication-running-parts(parts, title) = {
  for part in parts {
    if part.kind == "text" { text(part.text) }
    else if part.kind == "documentTitle" { text(title) }
    else if part.kind == "chapterTitle" { publication-chapter-title() }
    else if part.kind == "pageNumber" { context counter(page).display("1") }
    else if part.kind == "pageCount" { context counter(page).final().first() }
  }
}

#let publication-running-line(line, title) = context {
  if line.showOnFirstPage or counter(page).get().first() > 1 {
    let style = publication-style("footnote")
    set text(font: style.fontFamily, size: style.fontSizePt * 1pt, fill: rgb(publication-theme.colors.muted))
    grid(columns: (1fr, 1fr, 1fr), align: (left, center, right),
      publication-running-parts(line.left, title), publication-running-parts(line.center, title), publication-running-parts(line.right, title))
  }
}

#let publication-document(title: "", language: "en", body) = {
  let page-style = publication-theme.page
  let base = publication-style("body")
  set document(title: title)
  set text(lang: language, font: base.fontFamily, size: base.fontSizePt * 1pt, fill: rgb(base.color),
    weight: if base.fontWeight == "bold" { 700 } else { 400 }, style: if base.italic { "italic" } else { "normal" },
    top-edge: 0.8em, bottom-edge: -0.2em)
  set par(leading: (base.lineHeight - 1) * base.fontSizePt * 1pt, spacing: 0pt, justify: base.alignment == "justify")
  set align(publication-align(base.alignment))
  // Code color belongs to the portable theme in every target.
  set raw(theme: none)
  set page(width: page-style.widthPt * 1pt, height: page-style.heightPt * 1pt,
    margin: (top: page-style.marginTopPt * 1pt, right: page-style.marginRightPt * 1pt,
      bottom: page-style.marginBottomPt * 1pt, left: page-style.marginLeftPt * 1pt),
    fill: rgb(publication-theme.colors.paper),
    header: publication-running-line(page-style.header, title), footer: publication-running-line(page-style.footer, title),
    header-ascent: (page-style.marginTopPt - page-style.headerDistancePt) * 1pt,
    footer-descent: (page-style.marginBottomPt - page-style.footerDistancePt) * 1pt)
  show heading: it => publication-block("heading" + str(calc.min(it.level, 6)), it)
  show link: it => publication-text("link", it)
  show raw.where(block: true): it => publication-text("code", it)
  show raw.where(block: false): it => publication-inline("inlineCode", it)
  show figure.caption: it => publication-block("caption", it)
  show footnote.entry: it => publication-block("footnote", it)
  body
}
