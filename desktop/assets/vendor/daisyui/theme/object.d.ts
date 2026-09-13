interface Theme {
  "color-scheme": string
  "--color-base-100": string
  "--color-base-200": string
  "--color-base-300": string
  "--color-base-content": string
  "--color-primary": string
  "--color-primary-content": string
  "--color-secondary": string
  "--color-secondary-content": string
  "--color-accent": string
  "--color-accent-content": string
  "--color-neutral": string
  "--color-neutral-content": string
  "--color-info": string
  "--color-info-content": string
  "--color-success": string
  "--color-success-content": string
  "--color-warning": string
  "--color-warning-content": string
  "--color-error": string
  "--color-error-content": string
  "--radius-selector": string
  "--radius-field": string
  "--radius-box": string
  "--size-selector": string
  "--size-field": string
  "--border": string
  "--depth": string
  "--noise": string
}


interface Themes {
  dim: Theme
  nord: Theme
  fantasy: Theme
  cmyk: Theme
  light: Theme
  pastel: Theme
  acid: Theme
  wireframe: Theme
  coffee: Theme
  night: Theme
  cupcake: Theme
  synthwave: Theme
  sunset: Theme
  winter: Theme
  garden: Theme
  luxury: Theme
  retro: Theme
  aqua: Theme
  bumblebee: Theme
  dracula: Theme
  corporate: Theme
  lofi: Theme
  autumn: Theme
  dark: Theme
  abyss: Theme
  valentine: Theme
  halloween: Theme
  business: Theme
  silk: Theme
  forest: Theme
  black: Theme
  cyberpunk: Theme
  emerald: Theme
  lemonade: Theme
  caramellatte: Theme
  [key: string]: Theme
}

declare const themes: Themes
export default themes