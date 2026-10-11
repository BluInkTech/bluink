// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

export interface BundledFont {
	id: string
	family: string
	category: 'Serif' | 'Sans' | 'Mono' | 'International'
}
export const fonts: readonly BundledFont[]
export function fontStylesheet(id: string): string
