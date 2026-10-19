/** The handful of controls the settings UI is built from. */
import { useEffect, useId, useRef, useState, type ReactNode } from "react";

import { acceleratorFromEvent, cx, prettyAccelerator } from "../lib/util";

/* ------------------------------------------------------------------ structure */

export function Row({
  title,
  hint,
  children,
  top,
}: {
  title: ReactNode;
  hint?: ReactNode;
  children?: ReactNode;
  top?: boolean;
}) {
  return (
    <div className={cx("row", top && "row-top")}>
      <div className="row-main">
        <div className="row-title">{title}</div>
        {hint ? <div className="row-hint">{hint}</div> : null}
      </div>
      {children ? <div className="row-control">{children}</div> : null}
    </div>
  );
}

export function Field({
  label,
  hint,
  children,
}: {
  label: ReactNode;
  hint?: ReactNode;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <div>
      <label className="field-label" htmlFor={id}>
        {label}
      </label>
      {children}
      {hint ? <div className="field-hint">{hint}</div> : null}
    </div>
  );
}

export function Card({
  title,
  note,
  children,
  actions,
}: {
  title?: ReactNode;
  note?: ReactNode;
  children: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <section className="card">
      {title ? (
        <header className="card-head" style={{ display: "flex", gap: 10, alignItems: "center" }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div className="card-title">{title}</div>
            {note ? <div className="card-note">{note}</div> : null}
          </div>
          {actions}
        </header>
      ) : null}
      <div className="card-body">{children}</div>
    </section>
  );
}

/* --------------------------------------------------------------------- inputs */

export function Switch({
  checked,
  onChange,
  disabled,
  title,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  title?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={title}
      title={title}
      className="switch"
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}

export function Slider({
  value,
  min,
  max,
  step = 1,
  onChange,
  format,
  disabled,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (next: number) => void;
  format?: (value: number) => string;
  disabled?: boolean;
}) {
  return (
    <>
      <input
        className="range"
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      <span className="range-value">{format ? format(value) : value}</span>
    </>
  );
}

export function TextInput({
  value,
  onChange,
  placeholder,
  mono,
  type = "text",
  list,
  onBlur,
}: {
  value: string;
  onChange: (next: string) => void;
  placeholder?: string;
  mono?: boolean;
  type?: "text" | "url" | "password";
  list?: string;
  onBlur?: () => void;
}) {
  return (
    <input
      className={cx("input", mono && "input-mono")}
      type={type}
      value={value}
      list={list}
      spellCheck={false}
      autoComplete="off"
      placeholder={placeholder}
      onChange={(event) => onChange(event.target.value)}
      onBlur={onBlur}
    />
  );
}

export function SecretInput({
  value,
  onChange,
  placeholder,
  showLabel,
  hideLabel,
}: {
  value: string;
  onChange: (next: string) => void;
  placeholder?: string;
  showLabel: string;
  hideLabel: string;
}) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="input-group">
      <TextInput value={value} onChange={onChange} placeholder={placeholder} mono type={visible ? "text" : "password"} />
      <button type="button" className="btn btn-sm" onClick={() => setVisible((v) => !v)}>
        {visible ? hideLabel : showLabel}
      </button>
    </div>
  );
}

export function TextArea({
  value,
  onChange,
  placeholder,
  rows = 3,
}: {
  value: string;
  onChange: (next: string) => void;
  placeholder?: string;
  rows?: number;
}) {
  return (
    <textarea
      className="textarea"
      value={value}
      rows={rows}
      spellCheck={false}
      placeholder={placeholder}
      onChange={(event) => onChange(event.target.value)}
    />
  );
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  disabled,
}: {
  value: T;
  options: Array<{ value: T; label: string; disabled?: boolean }>;
  onChange: (next: T) => void;
  disabled?: boolean;
}) {
  return (
    <select
      className="select"
      style={{ width: "auto", minWidth: 150 }}
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(event.target.value as T)}
    >
      {options.map((option) => (
        <option key={option.value} value={option.value} disabled={option.disabled}>
          {option.label}
        </option>
      ))}
    </select>
  );
}

export function OptionCard({
  checked,
  onSelect,
  title,
  desc,
  disabled,
}: {
  checked: boolean;
  onSelect: () => void;
  title: ReactNode;
  desc?: ReactNode;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={checked}
      className="option"
      disabled={disabled}
      onClick={onSelect}
    >
      <span className="dot" />
      <span style={{ minWidth: 0 }}>
        <span className="option-title">{title}</span>
        {desc ? <span className="option-desc" style={{ display: "block" }}>{desc}</span> : null}
      </span>
    </button>
  );
}

/* ------------------------------------------------------------------- hotkeys */

export function HotkeyRecorder({
  value,
  recordingLabel,
  onCommit,
}: {
  value: string;
  recordingLabel: string;
  onCommit: (accelerator: string) => void;
}) {
  const [recording, setRecording] = useState(false);
  const [candidate, setCandidate] = useState<string | null>(null);
  const held = useRef(false);

  useEffect(() => {
    if (!recording) return;

    const onKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setRecording(false);
        setCandidate(null);
        return;
      }
      const accelerator = acceleratorFromEvent(event);
      if (accelerator) {
        setCandidate(accelerator);
        held.current = true;
      }
    };

    const onKeyUp = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      // Commit once the non-modifier key is released, so the user can see what
      // they pressed before it is applied.
      if (held.current && candidate && event.code === candidate.split("+").pop()) {
        held.current = false;
        setRecording(false);
        onCommit(candidate);
      }
    };

    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
    };
  }, [recording, candidate, onCommit]);

  return (
    <button
      type="button"
      className="recorder"
      data-recording={recording}
      onClick={() => {
        setCandidate(null);
        setRecording(true);
      }}
    >
      {recording ? (
        <span style={{ color: "var(--text-soft)" }}>
          {candidate ? prettyAccelerator(candidate) : recordingLabel}
        </span>
      ) : (
        <span>
          {value
            .split("+")
            .map((part) => (
              <kbd key={part}>{prettyAccelerator(part)}</kbd>
            ))
            .reduce<ReactNode[]>((acc, node, index) => (index ? [...acc, " ", node] : [node]), [])}
        </span>
      )}
    </button>
  );
}
