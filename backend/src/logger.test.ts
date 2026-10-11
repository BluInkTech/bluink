// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import { PassThrough } from 'node:stream'
import test from 'node:test'
import { styleText } from 'node:util'
import {
	type DiagnosticEvent,
	Logger,
	setBackendDiagnostics,
	setServerConsoleActive,
} from './logger.js'

const event: DiagnosticEvent = {
	timestampMs: 1,
	level: 'debug',
	target: 'bluink.test',
	name: 'sample_operation',
	message: 'span completed',
	fields: { durationMs: 4.2 },
}

function capture(terminal = false) {
	const stream = Object.assign(new PassThrough(), {
		isTTY: terminal,
		getColorDepth: () => 8,
	})
	let text = ''
	stream.on('data', (chunk: Buffer) => {
		text += chunk.toString()
	})
	return {
		stream,
		get text() {
			return text
		},
	}
}

test('CLI errors stay on stderr and leave stdout unstyled', () => {
	const stdout = capture(true)
	const stderr = capture()
	const logger = new Logger({
		stream: stdout.stream,
		diagnosticStream: stderr.stream,
		useColors: false,
	})
	logger.info('Usage: bluink server')
	logger.newLine()
	logger.error('The output already exists.')
	assert.equal(stdout.text, 'Usage: bluink server\n\n')
	assert.equal(stderr.text, '❌ The output already exists.\n')
})

test('CLI errors use stderr terminal colour capabilities', () => {
	const stderr = capture(true)
	const logger = new Logger({ diagnosticStream: stderr.stream })
	logger.error('Native backend unavailable')
	assert.equal(
		stderr.text,
		`❌ ${styleText('red', 'Native backend unavailable', { stream: stderr.stream })}\n`,
	)
})

test('host diagnostics remain available without the native sink', () => {
	const stderr = capture()
	const logger = new Logger({ diagnosticStream: stderr.stream })
	logger.diagnostic(event)
	assert.deepEqual(JSON.parse(stderr.text), event)
})

test('the native sink receives host events once and owns filtering', (t) => {
	const events: DiagnosticEvent[] = []
	setBackendDiagnostics((event) => events.push(event))
	t.after(() => setBackendDiagnostics(undefined))
	const stderr = capture()
	const logger = new Logger({ diagnosticStream: stderr.stream })
	logger.diagnostic(event)
	assert.deepEqual(events, [event])
	assert.equal(stderr.text, '')
})

test('errors use native diagnostics while the server console owns the terminal', (t) => {
	const events: DiagnosticEvent[] = []
	setBackendDiagnostics((event) => events.push(event))
	setServerConsoleActive(true)
	t.after(() => {
		setBackendDiagnostics(undefined)
		setServerConsoleActive(false)
	})
	const stderr = capture()
	const logger = new Logger({ diagnosticStream: stderr.stream })
	logger.error('Browser launch failed')
	assert.equal(stderr.text, '')
	assert.equal(events.length, 1)
	assert.equal(events[0]?.target, 'bluink.host')
	assert.equal(events[0]?.level, 'error')
	assert.deepEqual(events[0]?.fields, { error: 'Browser launch failed' })
})

test('a failed native sink falls back to stderr without losing the event', (t) => {
	setBackendDiagnostics(() => {
		throw new Error('Native diagnostics stopped')
	})
	t.after(() => setBackendDiagnostics(undefined))
	const stdout = capture()
	const stderr = capture()
	const logger = new Logger({
		stream: stdout.stream,
		diagnosticStream: stderr.stream,
	})
	logger.diagnostic(event)
	assert.equal(stdout.text, '')
	assert.deepEqual(JSON.parse(stderr.text), event)
})
