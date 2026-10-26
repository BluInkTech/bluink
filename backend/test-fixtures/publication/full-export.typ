#heading(level: 1)[#text("Heading one & <special>")]#label("block-heading-one")

#heading(level: 2)[#text("Heading two")]#label("block-heading-two")

#heading(level: 3)[#text("Heading three")]#label("block-heading-three")

#heading(level: 4)[#text("Heading four")]#label("block-heading-four")

#heading(level: 5)[#text("Heading five")]#label("block-heading-five")

#heading(level: 6)[#text("Heading six")]#label("block-heading-six")

#publication-par(anchor: "block-all-marks")[#text("Plain & <safe> ")#strong[#text("bold")]#emph[#text(" italic")]#strong[#super[#text(" raised bold")]]#underline[#text(" underline")]#strike[#text(" strike")]#raw(" x < y && z")#super[#text("2")]#sub[#text("O")]#link("https://example.com/?a=1&b=2")[#text(" External")]#link(label("chapter-two"))[#text(" Internal")]#link(label("block-heading-one"))[#text(" Anchor")]#linebreak()#text("Writer & editor")#footnote[#strong[#text("Rich footnote")]#link("https://example.com/source")[#text(" source")]]#label("reference-1")#footnote[#emph[#text("Inline citation")]]#label("reference-2")#footnote[#strong[#text("Shared source")]#link("https://example.com/source")[#text(" online")]]#label("reference-3")#footnote(label("reference-3"))]

#publication-par()[]

#publication-block("quote", anchor: "block-quote")[#publication-par()[#text("A quoted paragraph.")]

]

#publication-block("list")[#list(
[#publication-par()[#text("Bullet one")]

],
[#publication-par()[#text("Bullet two")]

],
)]

#publication-block("list")[#enum(start: 7,
[#publication-par()[#text("Number seven")]

],
[#publication-par()[#text("Number eight")]

],
)]

#publication-figure(kind: table, caption: [#text("Table caption")], anchor: "block-table-figure")[#publication-block("table")[#table(columns: (20fr, 30fr, 50fr,),
publication-cell(column: 0, row: 0, colspan: 1, rowspan: 2, header: true)[#publication-par()[#text("Two rows")]

],
publication-cell(column: 1, row: 0, colspan: 2, rowspan: 1, header: true)[#publication-par()[#text("Two columns")]

],
publication-cell(column: 1, row: 1, colspan: 1, rowspan: 1, header: false)[#publication-par()[#text("A")]

],
publication-cell(column: 2, row: 1, colspan: 1, rowspan: 1, header: false)[#publication-par()[#text("B")]

],
)]

]

#publication-figure(kind: image, caption: [#text("Image caption")], anchor: "block-image-figure")[#publication-image("assets/pixel.png","One transparent pixel", width: 48pt, height: 36pt,)

]

#publication-image("assets/pixel.png","Uncaptioned image", width: 24pt,)

#publication-figure(kind: raw, caption: [#text("Code caption")], anchor: "block-code-figure")[#publication-code("fn main() {\n    println!(\"# <literal>\");\n}", language: "rust")

]

#publication-code("plain code", language: "plain")

#publication-rule()

#publication-block("embed")[#publication-par()[#strong[#text("Shared source")]#link("https://example.com/source")[#text(" online")]]

]

#publication-block("boxInfo")[#publication-block("boxHeading")[#text("Information")]
#publication-par()[#text("Info body")]

]

#publication-block("boxAbstract")[#publication-par()[#text("Abstract body")]

]

#publication-block("boxNote")[#publication-par()[#text("Note body")]

]

#publication-block("boxTip")[#publication-block("list")[#list(
[#publication-par()[#text("Tip list")]

],
)]

]

#publication-block("boxSuccess")[#publication-par()[#text("Success body")]

]

#publication-block("boxImportant")[#publication-par()[#text("Important body")]

]

#publication-block("boxWarning")[#publication-par()[#text("Warning body")]

]

#publication-block("boxError")[#publication-par()[#text("Error body")]

]

#publication-block("boxQuestion")[#publication-par()[#text("Question body")]

]

#publication-block("boxExample")[#publication-par()[#text("Example body")]

]
