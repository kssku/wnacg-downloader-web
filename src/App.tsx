import { defineComponent } from 'vue'
import AppContent from './AppContent.tsx'
import {
  NConfigProvider,
  NModalProvider,
  NNotificationProvider,
  NMessageProvider,
  GlobalThemeOverrides,
} from 'naive-ui'
import { palette, radii } from './design-tokens'

export default defineComponent({
  name: 'App',
  setup() {
    const themeOverrides: GlobalThemeOverrides = {
      common: {
        primaryColor: palette.primary.DEFAULT,
        primaryColorHover: palette.primary.hover,
        primaryColorPressed: palette.primary.pressed,
        primaryColorSuppl: palette.primary.suppl,
        borderRadius: radii.md,
        borderRadiusSmall: radii.sm,
        heightMedium: '32px',
      },
      Button: {
        paddingSmall: '0 8px',
        paddingMedium: '0 12px',
      },
      Radio: {
        buttonColorActive: palette.primary.DEFAULT,
        buttonTextColorActive: palette.neutral.onPrimary,
      },
      Dropdown: {
        borderRadius: '5px',
        padding: '6px 2px',
        optionColorHover: palette.primary.DEFAULT,
        optionTextColorHover: palette.neutral.onPrimary,
        optionHeightMedium: '28px',
      },
    }

    return () => (
      <NConfigProvider theme-overrides={themeOverrides}>
        <NModalProvider>
          <NNotificationProvider placement="bottom-right" max={3}>
            <NMessageProvider>
              <AppContent />
            </NMessageProvider>
          </NNotificationProvider>
        </NModalProvider>
      </NConfigProvider>
    )
  },
})
