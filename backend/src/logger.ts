// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import type { Writable } from 'node:stream'
import { styleText } from 'node:util'

export type DiagnosticEvent = {
	timestampMs: number
	level: 'trace' | 'debug' | 'info' | 'warn' | 'error'
	target: string
	name?: string
	message?: string
	fields?: Record<string, unknown>
	spans?: string[]
	durationMs?: number
}

type DiagnosticSink = (event: DiagnosticEvent) => void

type LoggerOptions = {
	stream?: Writable
	diagnosticStream?: Writable
	useColors?: boolean
}

let backendDiagnostics: DiagnosticSink | undefined
let serverConsoleActive = false

export function setServerConsoleActive(active: boolean): void {
	serverConsoleActive = active
}

export function isServerConsoleActive(): boolean {
	return serverConsoleActive
}

/** Rust owns diagnostic filtering, formatting, capture and shutdown. */
export function setBackendDiagnostics(sink: DiagnosticSink | undefined): void {
	backendDiagnostics = sink
}

/** CLI output and host-event forwarding; application logging belongs to Rust. */
export class Logger {
	readonly #output: Writable
	readonly #diagnosticOutput: Writable
	readonly #useColors: boolean

	constructor(options: LoggerOptions = {}) {
		this.#output = options.stream ?? process.stdout
		this.#diagnosticOutput = options.diagnosticStream ?? process.stderr
		this.#useColors = options.useColors ?? true
	}

	error(message: string): void {
		if (serverConsoleActive) {
			this.diagnostic({
				timestampMs: Date.now(),
				level: 'error',
				target: 'bluink.host',
				name: 'host_error',
				fields: { error: message },
			})
			return
		}
		const text = this.#useColors
			? styleText('red', message, { stream: this.#diagnosticOutput })
			: message
		this.#diagnosticOutput.write(`❌ ${text}\n`)
	}

	info(message: string): void {
		this.#output.write(`${message}\n`)
	}

	newLine(): void {
		this.#output.write('\n')
	}

	diagnostic(event: DiagnosticEvent): void {
		if (backendDiagnostics) {
			try {
				backendDiagnostics(event)
				return
			} catch {
				// Preserve the event if the native sink is unavailable or has stopped.
			}
		}
		// Bootstrap fallback only; do not duplicate Rust's diagnostic renderer.
		this.#diagnosticOutput.write(`${JSON.stringify(event)}\n`)
	}
}

export default new Logger()
