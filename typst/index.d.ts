// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

export const upstreamVersion: string
export const targets: Readonly<Record<string, string>>
export function typstBinary(platform?: string, arch?: string): string
