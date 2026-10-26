// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk
'use strict'

const path = require('node:path')

const artifacts = {
  'win32-x64': 'bluink_runtime.win32-x64-msvc.node',
  'linux-x64': 'bluink_runtime.linux-x64-gnu.node',
  'darwin-arm64': 'bluink_runtime.darwin-arm64.node',
}

const target = `${process.platform}-${process.arch}`
const artifact = artifacts[target]

if (!artifact) {
  throw new Error(
    `Bluink's Rust runtime is not available for ${target}. ` +
      'Supported platforms are win32-x64, linux-x64, and darwin-arm64.',
  )
}

const mode = process.env.BLUINK_NATIVE_MODE ?? 'release'
if (!['development', 'release'].includes(mode)) {
  throw new Error(`Unknown BLUINK_NATIVE_MODE: ${mode}. Choose development or release.`)
}
module.exports = require(path.join(__dirname, mode === 'development' ? 'development' : '', artifact))
