/*
 * Heroicons v2 as CSS mask classes: `<span class="hero-cog-6-tooth">` rather than an
 * inline <svg> or a JS import. Adapted from the generator Phoenix ships, with one
 * difference that matters here — it reads a *curated* set under ./heroicons rather than
 * the full 1288-icon package, because this repo vendors what the console uses instead of
 * downloading a dependency at build time.
 *
 * To add an icon: copy its .svg out of the heroicons package into the matching directory
 * below and use `hero-<name>`. Nothing else needs editing.
 */
const plugin = require("tailwindcss/plugin")
const fs = require("fs")
const path = require("path")

module.exports = plugin(function ({ matchComponents, theme }) {
  let iconsDir = path.join(__dirname, "heroicons")
  let values = {}
  let icons = [
    ["", "/24/outline"],
    ["-solid", "/24/solid"],
    ["-mini", "/20/solid"],
    ["-micro", "/16/solid"],
  ]
  icons.forEach(([suffix, dir]) => {
    let full = path.join(iconsDir, dir)
    if (!fs.existsSync(full)) return
    fs.readdirSync(full).forEach((file) => {
      let name = path.basename(file, ".svg") + suffix
      values[name] = { name, fullPath: path.join(full, file) }
    })
  })
  matchComponents(
    {
      hero: ({ name, fullPath }) => {
        let content = fs.readFileSync(fullPath).toString().replace(/\r?\n|\r/g, "")
        content = encodeURIComponent(content)
        let size = theme("spacing.6")
        if (name.endsWith("-mini")) {
          size = theme("spacing.5")
        } else if (name.endsWith("-micro")) {
          size = theme("spacing.4")
        }
        return {
          [`--hero-${name}`]: `url('data:image/svg+xml;utf8,${content}')`,
          "-webkit-mask": `var(--hero-${name})`,
          mask: `var(--hero-${name})`,
          "mask-repeat": "no-repeat",
          "background-color": "currentColor",
          "vertical-align": "middle",
          display: "inline-block",
          width: size,
          height: size,
        }
      },
    },
    { values }
  )
})
