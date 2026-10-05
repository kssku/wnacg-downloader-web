import { defineComponent, PropType } from 'vue'

export default defineComponent({
  name: 'IconButton',
  props: {
    title: {
      type: String,
      default: '',
    },
    onClick: {
      type: Function as PropType<() => void>,
    },
  },
  setup(props, { slots }) {
    return () => (
      <div
        class="cursor-pointer p-1 rounded-lg flex items-center justify-between text-gray-6 hover:bg-primary hover:text-white active:bg-primary-pressed active:text-white"
        onClick={props.onClick}
        title={props.title}>
        {slots.default && slots.default()}
      </div>
    )
  },
})
