// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import catalogue from './catalogue.json' with { type: 'json' }

/** Stable catalogue shared by the application and publication generators. */
export const fonts = catalogue

export function fontStylesheet(id) {
	if (!fonts.some((font) => font.id === id))
		throw new Error(`Unknown bundled font: ${id}`)
	return `/fonts/${id}.css`
}
