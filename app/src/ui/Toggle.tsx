import { cx } from "./cx";
import s from "./Toggle.module.css";

interface Props {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
  disabled?: boolean;
  className?: string;
}

/** Off: dark track, grey knob left. On: white track, dark knob right (position and contrast, not hue). */
export function Toggle({ checked, onChange, label, disabled, className }: Props) {
  return (
    <button
      type="button" role="switch" aria-checked={checked} aria-label={label} disabled={disabled}
      className={cx(s.toggle, checked && s.on, className)}
      onClick={() => onChange(!checked)}
    >
      <span className={s.knob} />
    </button>
  );
}
