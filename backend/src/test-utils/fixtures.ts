// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import { mkdtemp, realpath, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { BluinkApplication } from '../../runtime/index.js'

/** Remove the disposable directory only after its callback releases resources. */
export async function withTempDir<T>(
	run: (directory: string) => Promise<T>,
): Promise<T> {
	const directory = await mkdtemp(join(tmpdir(), 'bluink-test-'))
	try {
		// macOS temporary paths can traverse /var -> /private/var. Match the
		// physical cwd used by child processes when asserting their output paths.
		return await run(await realpath(directory))
	} finally {
		await rm(directory, {
			recursive: true,
			force: true,
			maxRetries: 3,
			retryDelay: 30,
		})
	}
}

/** Seed host fixtures through Rust validation, without a second RPC client. */
export async function seedPages(
	application: BluinkApplication,
	pages: Array<{ title: string; text?: string }>,
): Promise<Array<{ id: string }>> {
	try {
		await application.initialize(false)
		const { url } = await application.startHttp(0)
		const items: Array<{ id: string }> = []
		for (const { title, text } of pages) {
			const response = await fetch(`${url}/api/create_item`, {
				method: 'POST',
				body: JSON.stringify({
					meta: { type: 'page', title },
					content: {
						type: 'doc',
						content: [
							{
								type: 'paragraph',
								...(text && { content: [{ type: 'text', text }] }),
							},
						],
					},
				}),
			})
			const item = (await response.json()) as { id: string }
			assert.ok(response.ok, JSON.stringify(item))
			assert.equal(typeof item.id, 'string')
			items.push({ id: item.id })
		}
		return items
	} finally {
		await application.stop()
	}
}
