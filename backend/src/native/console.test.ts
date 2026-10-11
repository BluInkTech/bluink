// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import test from 'node:test'
import { browserCommand, startRssReporting } from './console.js'
import { HostLifecycle } from './lifecycle.js'

test('RSS reports immediately, every five seconds, and stops without keeping the host alive', (context) => {
	context.mock.timers.enable({ apis: ['setInterval'] })
	const received: Array<number | null | undefined> = []
	let samples = 0
	const stop = startRssReporting(
		{ updateRss: (value) => received.push(value) },
		() => ++samples * 1024,
	)
	assert.deepEqual(received, [1024])
	context.mock.timers.tick(4999)
	assert.deepEqual(received, [1024])
	context.mock.timers.tick(1)
	assert.deepEqual(received, [1024, 2048])
	context.mock.timers.tick(5000)
	assert.deepEqual(received, [1024, 2048, 3072])
	stop()
	context.mock.timers.tick(10000)
	assert.equal(samples, 3)
})

test('an unavailable RSS sample is cleared and later samples recover', (context) => {
	context.mock.timers.enable({ apis: ['setInterval'] })
	const received: Array<number | null | undefined> = []
	let failed = true
	const stop = startRssReporting(
		{ updateRss: (value) => received.push(value) },
		() => {
			if (failed) throw new Error('Unavailable')
			return 4096
		},
	)
	assert.deepEqual(received, [null])
	failed = false
	context.mock.timers.tick(5000)
	assert.deepEqual(received, [null, 4096])
	stop()
})

test('browser launches use direct arguments on every supported platform', () => {
	const url = 'http://127.0.0.1:9000/?text=%24%28touch%20file%29'
	assert.deepEqual(browserCommand(url, 'darwin'), {
		command: 'open',
		args: [url],
	})
	assert.deepEqual(browserCommand(url, 'win32'), {
		command: 'rundll32',
		args: ['url.dll,FileProtocolHandler', url],
	})
	assert.deepEqual(browserCommand(url, 'linux'), {
		command: 'xdg-open',
		args: [url],
	})
	assert.throws(() => browserCommand('javascript:alert(1)'), /HTTP/)
})

test('terminal finalization follows pending work and runs despite cleanup failures', async () => {
	const lifecycle = new HostLifecycle()
	const order: string[] = []
	let complete: (() => void) | undefined
	lifecycle.register(async () => {
		order.push('application')
		complete?.()
		throw new Error('Injected cleanup failure')
	})
	lifecycle.registerFinalizer(async () => {
		order.push('terminal')
	})
	const work = lifecycle.run(async () => {
		await new Promise<void>((resolve) => {
			complete = resolve
		})
		order.push('pending work')
	})
	await Promise.resolve()
	const stopped = lifecycle.stop()
	assert.equal(lifecycle.stop(), stopped)
	await assert.rejects(stopped, /Backend cleanup failed/)
	await work
	assert.deepEqual(order, ['application', 'pending work', 'terminal'])
})
