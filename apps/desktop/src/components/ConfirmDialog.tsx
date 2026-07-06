import { useApp } from "../stores/useApp";
import OverlaySheet from "./OverlaySheet";

/** Reusable confirm dialog driven by `useApp.confirm`. Uses the shared
 *  OverlaySheet primitive but sits at z-[96], a tier above the other overlays
 *  (z-[95]), so it can appear over them. */
export default function ConfirmDialog() {
  const confirm = useApp((s) => s.confirm);

  // Declining (cancel button, backdrop, Escape) also fires the config's
  // onCancel hook, so consent flows can actively revert state on "no".
  const cancel = () => {
    const cfg = useApp.getState().confirm;
    useApp.getState().dismissConfirm();
    void cfg?.onCancel?.();
  };
  const run = async () => {
    const cfg = useApp.getState().confirm;
    useApp.getState().dismissConfirm();
    await cfg?.onConfirm();
  };

  return (
    <OverlaySheet
      open={!!confirm}
      onClose={cancel}
      zClassName="z-[96]"
      cardClassName="max-w-sm p-6"
    >
      {confirm && (
        <>
          <h2 className="text-title font-semibold text-ink">{confirm.title}</h2>
          <p className="mt-2 text-body text-subtle">{confirm.message}</p>

          <div className="mt-5 flex justify-end gap-2">
            <button
              autoFocus
              onClick={cancel}
              className="rounded-[8px] px-4 py-2 text-body font-medium text-ink hover:bg-ink/5"
            >
              {confirm.cancelLabel ?? "Cancel"}
            </button>
            <button
              onClick={() => void run()}
              className={`rounded-[8px] px-4 py-2 text-body font-medium text-white hover:opacity-90 ${
                confirm.destructive ? "bg-red-500" : "bg-accent"
              }`}
            >
              {confirm.confirmLabel}
            </button>
          </div>
        </>
      )}
    </OverlaySheet>
  );
}
