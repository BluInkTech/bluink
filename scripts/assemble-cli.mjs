// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { readNativeComponents } from './native-components.mjs'
import { contractHash, prepareCliBuild, verifyCliBuild } from './cli-build.mjs'
import { productionLock, verifyArchive } from './package-artifacts.mjs'
import {
	buildPlatformPackage,
	releaseNativeEntry,
} from './platform-package.mjs'
import { nativePackageVersion, publicIdentity } from './release-identity.mjs'
import { resourceAudit } from './resource-packages.mjs'
import { auditDirectory, cliFileAllowed } from './package-audit.mjs'
import {
	json,
	nativeDirectory as nativeOutputDirectory,
	npm,
	packageLauncher,
	root,
	sha256,
	targets,
	verifyNativeArtifacts,
	writeJson,
} from './release-utils.mjs'

/** Public assembly consumes fixed archives; it never compiles frontend source.
 * Each platform's addon becomes its own package, packed under `platforms` before
 * the main package pins it as an optional dependency. Returns the staged main
 * package directory and the packed platform archives. */
export async function buildPackage({
	version,
	metadata,
	nativeDirectory = nativeOutputDirectory('release'),
	dependencies,
	hostOnly = false,
	preparedDirectory,
	nativeSelection,
	nativeArchiveDirectory,
	destination = path.join(root, 'release'),
	platforms: platformDestination = `${destination}-platforms`,
} = {}) {
	if (!dependencies?.length)
		throw new Error(
			'CLI assembly requires verified Web and Fonts archives. Use npm run build in the private workspace or pass --dependencies in a public checkout.',
		)
	const source = await json(path.join(root, 'backend/package.json'))
	const productVersion = version ?? source.version
	const nativeVersion = nativePackageVersion(productVersion)
	const identity = publicIdentity(productVersion, metadata, !hostOnly)
	const buildDirectory = preparedDirectory ?? path.join(root, '.cli-build')
	let native = []
	const platforms = nativeSelection
		? await readNativeComponents(
				nativeSelection,
				nativeArchiveDirectory,
				identity.publicRepository,
			)
		: []
	if (!nativeSelection) {
		if (!preparedDirectory) {
			await npm(['run', 'contracts:check'])
			await prepareCliBuild(buildDirectory, {
				...identity,
				version: productVersion,
			})
		}
		const build = await verifyCliBuild(buildDirectory, {
			...identity,
			version: productVersion,
		})
		identity.toolchain = build.toolchain
		native = await verifyNativeArtifacts(nativeDirectory, undefined, hostOnly, {
			version: nativeVersion,
			publicCommit: identity.publicCommit,
		})
	}
	if (
		dependencies.length !== 2 ||
		new Set(dependencies.map((entry) => entry.name)).size !== 2 ||
		dependencies.some(
			(entry) => !['@bluink/web', '@bluink/fonts'].includes(entry.name),
		)
	)
		throw new Error('Supply exactly one Web and one Fonts archive.')
	dependencies = await Promise.all(
		dependencies.map((entry) =>
			verifyArchive(entry, resourceAudit(entry.name.split('/')[1])),
		),
	)
	const web = dependencies.find((entry) => entry.name === '@bluink/web')
	const inspected = web
	const fonts = dependencies.find((entry) => entry.name === '@bluink/fonts')
	if (
		inspected.metadata?.contractHash !== (await contractHash()) ||
		inspected.metadata?.fontsVersion !== fonts?.version ||
		inspected.metadata?.fontCatalogueHash !==
			sha256(await readFile(path.join(root, 'fonts/catalogue.json')))
	)
		throw new Error(
			'Web contract or font catalogue identity does not match this backend.',
		)
	const directory = destination
	await rm(directory, { recursive: true, force: true })
	await mkdir(directory, { recursive: true })
	for (const [from, to] of [
		['scripts/package-launcher.mjs', packageLauncher],
		['backend/LICENSE', 'LICENSE'],
		['backend/PACKAGE-README.md', 'README.md'],
		['backend/resources', 'vendor'],
	])
		await cp(path.join(root, from), path.join(directory, to), {
			recursive: true,
		})
	const notices = path.join(directory, 'THIRD_PARTY_NOTICES.txt')
	if (nativeSelection) await writeFile(notices, platforms[0].notices)
	else {
		await cp(path.join(buildDirectory, 'THIRD_PARTY_NOTICES.txt'), notices)
		await rm(platformDestination, { recursive: true, force: true })
		await mkdir(platformDestination, { recursive: true })
		for (const entry of native) {
			const target = targets.find((value) => value.rust === entry.target)
			const archive = await buildPlatformPackage(target, entry, {
				nativeDirectory,
				version: nativeVersion,
				license: source.license,
				identity,
				notices: path.join(buildDirectory, 'THIRD_PARTY_NOTICES.txt'),
				destination: platformDestination,
			})
			platforms.push({ ...archive, native: entry })
		}
	}
	const manifest = {
		name: source.name,
		version: productVersion,
		description: source.description,
		bin: { bluink: packageLauncher },
		engines: source.engines,
		license: source.license,
		homepage: 'https://bluink.app',
		...(identity.publicRepository && {
			repository: {
				type: 'git',
				url: `${identity.publicRepository}.git`,
				directory: 'backend',
			},
		}),
		dependencies: Object.fromEntries(
			dependencies.map((entry) => [entry.name, entry.version]),
		),
		optionalDependencies: Object.fromEntries(
			platforms.map((entry) => [entry.name, entry.version]),
		),
		publishConfig: {
			access: 'public',
			tag: identity.channel === 'stable' ? 'next' : identity.channel,
		},
		files: [...source.files, 'release.json'],
	}
	await writeJson(path.join(directory, 'package.json'), manifest)
	await writeJson(
		path.join(directory, 'npm-shrinkwrap.json'),
		productionLock(manifest, [...dependencies, ...platforms]),
	)
	await writeJson(path.join(directory, 'release.json'), {
		format: 'bluink.release',
		formatVersion: 1,
		...identity,
		name: manifest.name,
		version: manifest.version,
		distribution: hostOnly ? 'host-test' : 'all-platforms',
		...(nativeSelection && {
			nativeOrigin: {
				releaseVersion: nativeSelection.releaseVersion,
				publicCommit: nativeSelection.publicCommit,
			},
		}),
		contractHash: inspected.metadata.contractHash,
		frontend: {
			name: web.name,
			version: web.version,
			integrity: web.integrity,
			sha256: web.sha256,
		},
		dependencies: dependencies.map(({ name, version, integrity }) => ({
			name,
			version,
			integrity,
		})),
		native: platforms.map((entry) => releaseNativeEntry(entry, entry.native)),
	})
	await auditDirectory(directory, {
		allowed: cliFileAllowed,
		required: [
			packageLauncher,
			'npm-shrinkwrap.json',
			'THIRD_PARTY_NOTICES.txt',
		],
	})
	console.log(
		`Staged ${manifest.name}@${manifest.version} and ${platforms.map((entry) => entry.name).join(', ')}.`,
	)
	return { directory, platforms }
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	const { values } = (await import('node:util')).parseArgs({
		options: {
			version: { type: 'string' },
			dependencies: { type: 'string' },
			'host-only': { type: 'boolean', default: false },
			'native-dir': { type: 'string' },
		},
	})
	const records = await json(
		values.dependencies ??
			path.join(root, '.resource-packages/dependencies.json'),
	)
	await buildPackage({
		version: values.version,
		dependencies: records,
		hostOnly: values['host-only'],
		...(values['native-dir'] && {
			nativeDirectory: path.resolve(values['native-dir']),
		}),
	})
}
