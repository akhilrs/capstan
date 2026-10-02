import net from "node:net";

const MAX_RESPONSE_FRAME = 1_048_576;

type Request = {
  token: string;
  action: "status" | "inspect";
  id?: string;
};

export async function requestControl(
  socketPath: string,
  token: string,
  action: Request["action"],
  id?: string,
): Promise<unknown> {
  return await new Promise((resolve, reject) => {
    const socket = net.createConnection(socketPath);
    let bytes = Buffer.alloc(0);
    socket.once("connect", () =>
      socket.write(
        `${JSON.stringify({ token, action, ...(id ? { id } : {}) })}\n`,
      ),
    );
    socket.on("data", (chunk: Buffer) => {
      bytes = Buffer.concat([bytes, chunk]);
      if (bytes.length > MAX_RESPONSE_FRAME) {
        socket.destroy();
        reject(new Error("control response exceeds limit"));
        return;
      }
      const newline = bytes.indexOf(10);
      if (newline < 0) return;
      try {
        const response = JSON.parse(
          new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(
            bytes.subarray(0, newline),
          ),
        ) as { result?: unknown; error?: string };
        socket.end();
        if (response.error) reject(new Error(response.error));
        else resolve(response.result);
      } catch (error) {
        reject(error);
      }
    });
    socket.once("error", reject);
    socket.setTimeout(5_000, () => {
      socket.destroy();
      reject(new Error("control request timed out"));
    });
  });
}
