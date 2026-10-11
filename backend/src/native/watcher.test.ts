// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import test from 'node:test'
import { setTimeout } from 'node:timers/promises'
import { withTempDir } from '../test-utils/fixtures.js'
import { subscribeProject } from './watcher.js'

async function waitFor(predicate: () => boolean, probe?: () => Promise<void>) {
	const deadline = Date.now() + 3_000
	while (!predicate()) {
		assert.ok(Date.now() < deadline, 'OS watcher did not deliver the event')
		await probe?.()
		await setTimeout(30)
	}
}

test('watcher transport forwards optional filenames and isolates subscription and delivery failures', () => {
	const events: Array<string | null | undefined> = []
	let errors = 0
	let closed = 0
	const listeners: Array<
		(event: string, filename: string | Buffer | null) => void
	> = []
	const watchers: EventEmitter[] = []
	const stop = subscribeProject(
		{
			notifyFileChange: (filename) => {
				if (filename === 'items/failure')
					throw new Error('Receiver unavailable')
				events.push(filename)
			},
			reportWatcherError: () => {
				errors++
			},
		},
		['missing', 'items', 'properties'].map((prefix) => ({
			path: prefix,
			prefix,
		})),
		(directory, options, listener) => {
			assert.deepEqual(options, { recursive: false })
			if (directory === 'missing') throw new Error('Missing directory')
			listeners.push(listener)
			const watcher = Object.assign(new EventEmitter(), {
				close() {
					closed++
				},
			})
			watchers.push(watcher)
			return watcher
		},
	)
	assert.equal(errors, 1)
	listeners[0]!('change', 'note')
	listeners[1]!('rename', Buffer.from('field'))
	listeners[0]!('change', null)
	assert.deepEqual(events, ['items/note', 'properties/field', undefined])
	listeners[0]!('change', 'failure')
	watchers[0]!.emit('error', new Error('Watcher unavailable'))
	assert.equal(errors, 3)
	stop()
	stop()
	assert.equal(closed, 2)
	listeners[0]!('change', 'after-stop')
	watchers[1]!.emit('error', new Error('Late error'))
	assert.equal(errors, 3)
	assert.equal(events.length, 3)
})

test('real OS subscriptions stay shallow and stop delivering after disposal', {
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
		const events: Array<Array<string | null | undefined>> = roots.map(() => [])
		let errors = 0
		const stops = roots.map((root, index) =>
			subscribeProject(
				{
					notifyFileChange: (path) => {
						events[index]!.push(path)
					},
					reportWatcherError: () => {
						errors++
					},
				},
				[root],
			),
		)
		const stop = () => {
			for (const dispose of stops) dispose()
		}
		try {
			assert.equal(
				handles() - before,
				roots.length,
				'Nested directories must not allocate more subscriptions',
			)
			// FSEvents initialization is asynchronous and filenames are optional.
			// Probe until each subscription is ready; exact transport is tested above.
			let probe = 0
			await waitFor(
				() => events.every((received) => received.length > 0),
				async () => {
					await Promise.all(
						roots.map((root) =>
							writeFile(join(root.path, `probe-${probe++}`), 'item'),
						),
					)
				},
			)
			for (const [index, received] of events.entries())
				for (const filename of received)
					if (filename !== undefined && filename !== null)
						assert.ok(filename.startsWith(`${roots[index]!.prefix}/`))
			assert.equal(errors, 0)
			stop()
			stop()
			await waitFor(() => handles() === before)
			const delivered = events.flat().length
			await writeFile(join(roots[0]!.path, 'after-stop'), 'ignored')
			await setTimeout(100)
			assert.equal(events.flat().length, delivered)
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
			let probe = 0
			await waitFor(
				() => errors > 1,
				() => writeFile(join(directory, `field-${probe++}`), 'property'),
			)
		} finally {
			stop()
		}
	})
})
