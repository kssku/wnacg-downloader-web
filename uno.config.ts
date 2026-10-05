import {
  defineConfig,
  presetAttributify,
  presetIcons,
  presetTypography,
  presetUno,
  presetWebFonts,
  transformerDirectives,
  transformerVariantGroup,
} from 'unocss'
// 注意：这里必须带 .ts 扩展名 —— uno.config.ts 由 Node 的 ESM 解析器加载，
// 它不做 TS 的扩展名补全（不带扩展名会报 ERR_MODULE_NOT_FOUND，构建直接失败）。
import { palette } from './src/design-tokens.ts'

export default defineConfig({
  shortcuts: [
    // ...
  ],
  // 扫描范围：补上 .ts。
  //
  // 原因：UnoCSS 的 defaultPipelineInclude 是
  //   /\.(vue|svelte|[jt]sx|mdx?|astro|elm|php|phtml|html)($|\?)/
  // —— 有 [jt]sx（所以 .tsx 里的任意值类本来就能扫到），但没有独立的 ts，
  // 所以 .ts 文件整体不被扫描。
  //
  // 结果：写在 .ts 里的 arbitrary class 字面量（如 bg-[var(--x)]）扫不到，CSS 不生成。
  // 加这条补丁是为了将来在 design-tokens.ts 之类的地方写任意值类时不会被静默丢弃。
  //
  // 代价：扫到不认识的字符串不会生成 CSS，只是多花一点扫描时间；
  // 唯一的副作用是误匹配时多出几十字节用不到的 CSS。
  content: {
    pipeline: {
      include: [/\.(vue|svelte|[jt]sx|mdx?|astro|elm|php|phtml|html|ts)($|\?)/],
    },
  },
  theme: {
    colors: {
      // 唯一来源：src/design-tokens.ts
      // 不要在这里写字面量色值 —— 否则又会和 themeOverrides 分裂。
      primary: {
        DEFAULT: palette.primary.DEFAULT,
        hover: palette.primary.hover,
        pressed: palette.primary.pressed,
        suppl: palette.primary.suppl,
      },
    },
  },
  presets: [
    presetUno(),
    presetAttributify(),
    presetIcons(),
    presetTypography(),
    presetWebFonts({
      fonts: {
        // ...
      },
    }),
  ],
  transformers: [transformerDirectives(), transformerVariantGroup()],
})
