// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import test from 'node:test'
import { setTimeout } from 'node:timers/promises'
import { withTempDir } from '../test-utils/fixtures.js'
import { subscribeProject } from './watcher.js'

async function waitFor(predicate: () => boolean) {
	const deadline = Date.now() + 3_000
	while (!predicate()) {
		assert.ok(Date.now() < deadline, 'OS watcher did not deliver the event')
		await setTimeout(10)
	}
}

test('OS subscriptions stay shallow, forward root-relative paths and stop delivering after disposal', {
	timeout: 10_000,
}, async () => {
	await withTempDir(async (directory) => {
		const roots = ['items', 'properties'].map((prefix) => ({
			path: join(directory, prefix),
			prefix,
		}))
		for (const root of roots)
			await mkdir(join(root.path, 'nested/deep'), { recursive: true })
		const handles = () =>
			process
				.getActiveResourcesInfo()
				.filter((resource) => resource === 'FSEventWrap').length
		const before = handles()
		const events: Array<string | null | undefined> = []
		let errors = 0
		const stop = subscribeProject(
			{
				notifyFileChange: (path) => {
					events.push(path)
				},
				reportWatcherError: () => {
					errors++
				},
			},
			roots,
		)
		try {
			assert.equal(
				handles() - before,
				roots.length,
				'Nested directories must not allocate more subscriptions',
			)
			await writeFile(join(roots[0]!.path, 'note'), 'item')
			await writeFile(join(roots[1]!.path, 'field'), 'property')
			await waitFor(
				() =>
					events.includes('items/note') && events.includes('properties/field'),
			)
			assert.equal(errors, 0)
			stop()
			stop()
			await waitFor(() => handles() === before)
			const delivered = events.length
			await writeFile(join(roots[0]!.path, 'after-stop'), 'ignored')
			await setTimeout(100)
			assert.equal(events.length, delivered)
		} finally {
			stop()
		}
	})
})

test('subscription and event-delivery failures report errors without stopping other roots', {
	timeout: 10_000,
}, async () => {
	await withTempDir(async (directory) => {
		let errors = 0
		const stop = subscribeProject(
			{
				notifyFileChange: () => {
					throw new Error('Native receiver unavailable')
				},
				reportWatcherError: () => {
					errors++
				},
			},
			[
				{ path: join(directory, 'missing'), prefix: 'missing' },
				{ path: directory, prefix: 'properties' },
			],
		)
		try {
			assert.equal(errors, 1)
			await writeFile(join(directory, 'field'), 'property')
			await waitFor(() => errors > 1)
		} finally {
			stop()
		}
	})
})
