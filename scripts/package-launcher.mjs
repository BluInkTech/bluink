#!/usr/bin/env node
// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

// Published as @bluink/cli's bin/bluink.mjs. It must only import Node built-ins.
// Node only loads Bluink's addon and waits; every command runs in Rust.
import { createRequire } from 'node:module'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const minimumNode = 26

/** The npm package holding this platform's addon, or undefined if none is built. */
export function platformPackage({
	platform = process.platform,
	arch = process.arch,
	glibc = process.report?.getReport?.().header?.glibcVersionRuntime,
} = {}) {
	if (platform === 'win32' && arch === 'x64') return '@bluink/cli-win32-x64'
	if (platform === 'darwin' && arch === 'arm64')
		return '@bluink/cli-darwin-arm64'
	if (platform === 'linux' && arch === 'x64' && glibc)
		return '@bluink/cli-linux-x64-gnu'
	return undefined
}

function describe() {
	const glibc = process.report?.getReport?.().header?.glibcVersionRuntime
	return `${process.platform}/${process.arch}${process.platform === 'linux' ? (glibc ? ` (glibc ${glibc})` : ' (not glibc)') : ''}`
}

/** `BLUINK_ADDON` names a development build; otherwise use the platform package. */
function loadAddon(require) {
	const override = process.env.BLUINK_ADDON
	if (override) {
		const module = { exports: {} }
		process.dlopen(module, path.resolve(override))
		return module.exports
	}
	const name = platformPackage()
	if (!name) throw new Error(`Bluink has no build for ${describe()}.`)
	try {
		return require(name)
	} catch (error) {
		if (error?.code !== 'MODULE_NOT_FOUND') throw error
		throw new Error(
			`${name} is not installed. Reinstall Bluink without --omit=optional so npm installs the part for ${describe()}.`,
		)
	}
}

async function main() {
	const major = Number(process.versions.node.split('.')[0])
	if (major < minimumNode) {
		console.error(
			`Bluink needs Node.js ${minimumNode} or later; this is ${process.version}.`,
		)
		return 1
	}
	const packageRoot = path.resolve(
		path.dirname(fileURLToPath(import.meta.url)),
		'..',
	)
	let addon
	try {
		addon = loadAddon(createRequire(import.meta.url))
	} catch (error) {
		console.error(error instanceof Error ? error.message : String(error))
		return 1
	}
	// Node owns signals; each one asks Bluink for the same graceful stop.
	const signals = ['SIGINT', 'SIGTERM', 'SIGHUP']
	if (process.platform === 'win32') signals.push('SIGBREAK')
	for (const signal of signals) process.on(signal, () => addon.shutdown())
	return addon.run({ argv: process.argv.slice(2), packageRoot })
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
	process.exitCode = await main()
