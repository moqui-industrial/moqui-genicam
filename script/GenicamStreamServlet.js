/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

// The live MJPEG servlet: GET ?deviceId=...&connectionName=...&maxFrames=...&frameDelayMs=... answers
// GenicamServices.stream#LiveMjpeg. A servlet's response goes to the client as it is written, so each frame arrives
// when the service flushes it. The application declares it in its conf (README.md, Live MJPEG); it needs a signed-in
// user with permission on the service.
;({
    doGet(request, response) {
        if (!ec.user.userId) {
            response.sendError(401, "Authentication required")
            return
        }
        const number = name => (request.getParameter(name) == null || request.getParameter(name) === "" ? null
            : Number(request.getParameter(name)))
        ec.service.sync().name("moqui.genicam.GenicamServices.stream#LiveMjpeg").parameters({
            deviceId: request.getParameter("deviceId"), connectionName: request.getParameter("connectionName"),
            maxFrames: number("maxFrames"), frameDelayMs: number("frameDelayMs"),
        }).call()
        if (ec.message.hasError() && !response.isCommitted()) response.sendError(400, ec.message.errorsString)
    },
})
