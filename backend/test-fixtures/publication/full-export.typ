#publication-block("heading1")[#heading(level: 1)[#text("Heading one & <special>")]#label("block-heading-one")]

#publication-block("heading2")[#heading(level: 2)[#text("Heading two")]]

#publication-block("heading3")[#heading(level: 3)[#text("Heading three")]]

#publication-block("heading4")[#heading(level: 4)[#text("Heading four")]]

#publication-block("heading5")[#heading(level: 5)[#text("Heading five")]]

#publication-block("heading6")[#heading(level: 6)[#text("Heading six")]]

#publication-par()[#text("Plain & <safe> ")#strong[#text("bold")]#emph[#text(" italic")]#strong[#super[#text(" raised bold")]]#underline[#text(" underline")]#strike[#text(" strike")]#raw(" x < y && z")#super[#text("2")]#sub[#text("O")]#link("https://example.com/?a=1&b=2")[#text(" External")]#link(label("chapter-two"))[#text(" Internal")]#link(label("block-heading-one"))[#text(" Anchor")]#linebreak()#text("Writer & editor")#footnote[#strong[#text("Rich footnote")]#link("https://example.com/source")[#text(" source")]]\[#link(label("ref-1"))[1], #link(label("ref-2"))[2]\]]

#publication-par()[]

#publication-block("quote")[#publication-par()[#text("A quoted paragraph.")]

#publication-block("attribution")[#text("— ")#text("A reviewer")]
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

#publication-figure(kind: table, caption: [#text("Table caption")])[#publication-block("table")[#table(columns: (20fr, 30fr, 50fr,),
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

#publication-figure(kind: image, caption: [#text("Image caption")])[#publication-image("assets/pixel.png","One transparent pixel", width: 48pt, height: 36pt,)

]

#publication-image("assets/pixel.png","Uncaptioned image", width: 24pt,)

#publication-figure(kind: raw, caption: [#text("Code caption")])[#publication-code("fn main() {\n    println!(\"# <literal>\");\n}", language: "rust")

]

#publication-code("plain code", language: "plain")

#publication-rule()

#publication-block("embed")[#publication-par()[#strong[#text("Shared source")]#link("https://example.com/source")[#text(" online")]]

]

#publication-block("boxNote")[#publication-block("boxHeading")[#text("Information")]
#publication-par()[#text("Info body")]

]

#publication-block("box")[#publication-par()[#text("Abstract body")]

]

#publication-block("boxNote")[#publication-block("boxHeading")[#text("Note")]
#publication-par()[#text("Note body")]

]

#publication-block("boxTip")[#publication-block("boxHeading")[#text("Tip")]
#publication-block("list")[#list(
[#publication-par()[#text("Tip list")]

],
)]

]

#publication-block("boxTip")[#publication-block("boxHeading")[#text("Tip")]
#publication-par()[#text("Success body")]

]

#publication-block("boxImportant")[#publication-block("boxHeading")[#text("Important")]
#publication-par()[#text("Important body")]

]

#publication-block("boxWarning")[#publication-block("boxHeading")[#text("Warning")]
#publication-par()[#text("Warning body")]

]

#publication-block("boxCaution")[#publication-block("boxHeading")[#text("Caution")]
#publication-par()[#text("Error body")]

]

#publication-block("boxNote")[#publication-block("boxHeading")[#text("Note")]
#publication-par()[#text("Question body")]

]

#publication-block("box")[#publication-par()[#text("Example body")]

]

#publication-references[#publication-block("heading2")[#heading(level: 2)[#text("References")]]

#publication-reference-item("1", "ref-1")[#emph[#text("Inline citation")]]

#publication-reference-item("2", "ref-2")[#strong[#text("Shared source")]#link("https://example.com/source")[#text(" online")]]

]
