#import "theme-values.typ": *
#set page(paper: "a4", margin: (x: 2.2cm, y: 2.5cm), fill: paper)
#set text(font: body-font, size: body-size, fill: ink)
#set par(leading: leading)
#show heading: set text(font: heading-font, fill: accent)
#show link: set text(fill: link-color)
#include "content.typ"
