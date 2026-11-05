// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

// Published as @bluink/cli's install.mjs. It must only import Node built-ins.
import { createHash } from 'node:crypto'
import { realpathSync } from 'node:fs'
import { chmod, copyFile, readFile, rename, rm } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const bin = 'bin/bluink.exe'

async function digest(file) {
	return createHash('sha256')
		.update(await readFile(file))
		.digest('hex')
}

/** Put this platform's executable at npm's bin path. npm's Windows shim runs
 * the packaged bin/bluink.exe directly, so Windows needs no change. Re-running
 * is safe: matching bytes are left in place and only their mode is set.
 */
export async function installExecutable(
	packageRoot,
	{ platform = process.platform, arch = process.arch } = {},
) {
	if (platform === 'win32') return undefined
	const release = JSON.parse(
		await readFile(path.join(packageRoot, 'release.json'), 'utf8'),
	)
	const native = release.native ?? []
	const entry = native.find(
		(value) => value.file === `bluink-${platform}-${arch}`,
	)
	if (!entry)
		throw new Error(
			`Bluink ${release.version} does not support ${platform}/${arch}. This package contains executables for ${
				native
					.map((value) => value.file.replace(/^bluink-|\.exe$/g, ''))
					.join(', ') || 'no platform'
			}.`,
		)
	const destination = path.join(packageRoot, bin)
	const installed = await digest(destination).catch((error) => {
		if (error.code === 'ENOENT') return undefined
		throw error
	})
	if (installed !== entry.sha256) {
		const source = path.join(packageRoot, entry.path)
		const actual = await digest(source).catch((error) => {
			if (error.code === 'ENOENT') return undefined
			throw error
		})
		if (actual !== entry.sha256)
			throw new Error(
				`${entry.path} is ${actual ? 'not the executable recorded in release.json' : 'missing'}. Reinstall @bluink/cli.`,
			)
		// Replace by rename so an interrupted copy never leaves a partial bin.
		const temporary = `${destination}.${process.pid}.tmp`
		try {
			await copyFile(source, temporary)
			await chmod(temporary, 0o755)
			await rename(temporary, destination)
		} finally {
			await rm(temporary, { force: true })
		}
	}
	await chmod(destination, 0o755)
	return destination
}

const self = fileURLToPath(import.meta.url)
if (process.argv[1] && realpathSync(process.argv[1]) === self) {
	try {
		await installExecutable(path.dirname(self))
	} catch (error) {
		console.error(`@bluink/cli: ${error.message}`)
		process.exitCode = 1
	}
}
