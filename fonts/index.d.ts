// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

export { fonts, fontStylesheet, type BundledFont } from './catalogue.js'
export const version: string
export function fontDirectory(format: 'web' | 'pdf' | 'docx'): string
