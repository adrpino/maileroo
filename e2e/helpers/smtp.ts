import net from "net";

export async function deliverInbound(
  smtpAddr: string,
  from: string,
  to: string,
  rawMime: string,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const parts = smtpAddr.split(":");
    const host = parts[0];
    const port = parseInt(parts[1], 10);
    const socket = net.connect({ port, host });
    
    let buffer = "";
    let step = 0; // 0: greeting, 1: EHLO, 2: MAIL FROM, 3: RCPT TO, 4: DATA, 5: BODY, 6: QUIT

    socket.on("data", (chunk) => {
      buffer += chunk.toString();
      
      while (true) {
        const lineEnd = buffer.indexOf("\r\n");
        if (lineEnd === -1) break;
        
        const line = buffer.substring(0, lineEnd);
        buffer = buffer.substring(lineEnd + 2);
        
        if (step === 0 && line.startsWith("220")) {
          socket.write("EHLO e2e.test\r\n");
          step = 1;
        } else if (step === 1) {
          // EHLO response can be multi-line (ends with "250 ")
          if (line.startsWith("250 ")) {
            socket.write(`MAIL FROM:<${from}>\r\n`);
            step = 2;
          }
        } else if (step === 2) {
          if (line.startsWith("250")) {
            socket.write(`RCPT TO:<${to}>\r\n`);
            step = 3;
          } else if (line.match(/^[45]\d{2}/)) {
            socket.destroy();
            reject(new Error(`MAIL FROM rejected: ${line}`));
            return;
          }
        } else if (step === 3) {
          if (line.startsWith("250")) {
            socket.write("DATA\r\n");
            step = 4;
          } else if (line.match(/^[45]\d{2}/)) {
            socket.destroy();
            reject(new Error(`RCPT TO rejected: ${line}`));
            return;
          }
        } else if (step === 4) {
          if (line.startsWith("354")) {
            socket.write(rawMime + "\r\n.\r\n");
            step = 5;
          } else if (line.match(/^[45]\d{2}/)) {
            socket.destroy();
            reject(new Error(`DATA rejected: ${line}`));
            return;
          }
        } else if (step === 5) {
          if (line.startsWith("250")) {
            socket.write("QUIT\r\n");
            step = 6;
          } else if (line.match(/^[45]\d{2}/)) {
            socket.destroy();
            reject(new Error(`MIME data rejected: ${line}`));
            return;
          }
        } else if (step === 6) {
          if (line.startsWith("221") || line.startsWith("250")) {
            socket.end();
            resolve();
          }
        }
      }
    });

    socket.on("error", (err) => {
      socket.destroy();
      reject(err);
    });

    socket.on("close", () => {
      if (step < 6) {
        reject(new Error(`SMTP connection closed prematurely at step ${step}`));
      }
    });
  });
}
