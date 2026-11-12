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

// The logic of the services of GenicamServices.xml: the Groovy scripts of the Java version and the Groovy class
// org.moqui.genicam.GenicamUtil. The services call it with
// ec.resource.script('component://moqui-genicam/service/moqui/genicam/Genicam.js').execute(operation, context),
// after their XML Actions have found the Device and its DeviceConnection. The camera is the Rust tool GenicamClient
// (the Groovy class GenicamClient), the multi-camera process the tool GenicamMultiCameraRunner.
//
// The Python bridge ran with the ExecutionContext in JEP and wrote the DeviceContent rows and the error status of the
// Device itself; it now runs in a worker process without one, so those writes are here.

const DEFAULT_IMAGES_PATH = "runtime/genicam/images"
const DEFAULT_FRAMES_PATH = "runtime/genicam/frames"
const DEFAULT_VIDEOS_PATH = "runtime/genicam/videos"
const DEFAULT_SERVO_PATH = "runtime/genicam/servo"
const DEFAULT_TENSORS_PATH = "runtime/genicam/tensors"
const SUPPORTED_IMAGE_FORMATS = ["jpg", "png", "bmp"]
const SUPPORTED_VIDEO_CONTAINERS = ["avi", "mp4"]
const SUPPORTED_VIDEO_CODECS = ["MJPG", "XVID", "mp4v"]
// The name of the Tensors of acquire#GenICam3DFrame: clean#GenICamTensors removes only these.
const TENSOR_NAME_PREFIX = "GenICam 3D Frame - Camera "

const one = (entityName, field, value) => ec.entity.find(entityName).condition(field, value).useCache(false).one()
const blank = value => value == null || String(value).trim() === ""

// ---------- GenicamUtil: configuration ----------

// A default-property of MoquiConf.xml (or a system property or environment variable of the same name).
function configProperty(name, defaultValue) {
    const value = System.getProperty(name)
    return blank(value) ? defaultValue : String(value).trim()
}

function configured(value, propertyName, parse) {
    if (!blank(value)) return parse(value)
    const property = configProperty(propertyName, null)
    return property == null ? null : parse(property)
}
const configuredInteger = (value, propertyName) => configured(value, propertyName, v => parseInt(String(v), 10))
const configuredNumber = (value, propertyName) => configured(value, propertyName, v => Number(String(v)))
const configuredBoolean = (value, propertyName) =>
    configured(value, propertyName, v => (typeof v === "boolean" ? v : String(v).trim().toLowerCase() === "true"))

const isAbsolute = location => location.startsWith("/") || /^[A-Za-z]:[\\/]/.test(location)

// A location relative to the runtime directory ("runtime/genicam/images" or "genicam/images") as an absolute path.
function runtimePath(location) {
    if (blank(location)) return null
    location = String(location).trim()
    if (isAbsolute(location)) return location
    const runtime = String(ec.factory.getRuntimePath()).replace(/\\/g, "/").replace(/\/$/, "")
    return runtime + "/" + (location.startsWith("runtime/") ? location.substring(8) : location)
}

const configuredPath = (location, propertyName, defaultLocation) =>
    runtimePath(blank(location) ? configProperty(propertyName, defaultLocation) : location)

function imageFormat(value) {
    const configuredFormat = blank(value) ? configProperty("genicam.image.format", "jpg") : String(value)
    let format = configuredFormat.trim().toLowerCase()
    if (format === "jpeg") format = "jpg"
    if (!SUPPORTED_IMAGE_FORMATS.includes(format))
        throw new Error(`Unsupported image format ${configuredFormat}. Supported formats: ${SUPPORTED_IMAGE_FORMATS}`)
    return format
}

function videoContainer(value) {
    const configuredContainer = blank(value) ? configProperty("genicam.video.container", "avi") : String(value)
    const container = configuredContainer.trim().toLowerCase()
    if (!SUPPORTED_VIDEO_CONTAINERS.includes(container))
        throw new Error(`Unsupported video container ${configuredContainer}. Supported containers: ${SUPPORTED_VIDEO_CONTAINERS}`)
    return container
}

function videoCodec(value) {
    const configuredCodec = blank(value) ? configProperty("genicam.video.codec", "MJPG") : String(value)
    const codec = configuredCodec.trim()
    if (!SUPPORTED_VIDEO_CODECS.includes(codec))
        throw new Error(`Unsupported video codec ${configuredCodec}. Supported codecs: ${SUPPORTED_VIDEO_CODECS}`)
    return codec
}

// ---------- GenicamUtil: the camera ----------

// The serial number of a camera: serial=... in DeviceConnection.options, else PhysicalDevice.serialNumber, else the
// connection name.
function serialNumber(device, connection) {
    const physical = device ? one("moqui.device.PhysicalDevice", "deviceId", device.deviceId) : null
    if (!connection) return physical ? physical.serialNumber : null
    for (const option of String(connection.options || "").split(",")) {
        const [key, value] = option.split("=", 2).map(part => part.trim())
        if (key === "serial" && value) return value
    }
    if (physical && physical.serialNumber) return physical.serialNumber
    return connection.connectionName
}

// The connection must belong to the camera and name its GenTL driver.
function checkConnection(device, connection) {
    if (connection.deviceId !== device.deviceId)
        throw new Error(`The connection ${connection.connectionName} belongs to another device than ${device.deviceId}`)
    if (blank(connection.transportConfig))
        throw new Error("Connection transportConfig must specify the absolute path to the .cti GenTL driver.")
}

function camera(device, connection) {
    checkConnection(device, connection)
    return ec.getTool("GenicamClient", connection.transportConfig, serialNumber(device, connection), device.deviceId)
}

// What the bridge did on a camera failure (update_device_status_to_error): the Device goes to DbsErrorStop.
function markDeviceError(deviceId) {
    try {
        ec.service.sync().name("update#moqui.device.Device").parameters({deviceId, statusId: "DbsErrorStop"})
            .requireNewTransaction(true).call()
        ec.logger.info(`Updated Moqui Device ${deviceId} status to DbsErrorStop due to hardware/connection failure.`)
    } catch (error) {
        ec.logger.error(`Failed to update device status to DbsErrorStop: ${error}`)
    }
}

// Run a call of the camera; a failure marks the Device and is the error of the service.
function withCamera(device, connection, work) {
    const client = camera(device, connection)
    try {
        return work(client)
    } catch (error) {
        markDeviceError(device.deviceId)
        throw error
    }
}

// ---------- DeviceContent and Tensor records ----------

const contentType = location => (/\.(jpe?g|png|bmp)$/i.test(location) ? "DCntImage" : null)

// _store_device_content_reference of the bridge: an image file as a DeviceContent of the camera, found again by its
// location.
function storeDeviceContent(deviceId, location, description) {
    const contentTypeEnumId = contentType(location)
    if (!deviceId || blank(location) || !contentTypeEnumId) return null
    const contentLocation = String(location).replace(/\\/g, "/")
    const existing = ec.entity.find("moqui.device.DeviceContent").condition("deviceId", deviceId)
        .condition("contentLocation", contentLocation).useCache(false).one()
    const fields = {contentLocation, contentTypeEnumId, contentDate: ec.user.nowTimestamp}
    if (description) fields.description = description
    if (existing) {
        ec.service.sync().name("update#moqui.device.DeviceContent")
            .parameters({...fields, deviceContentId: existing.deviceContentId}).call()
        return existing.deviceContentId
    }
    const sequenceNum = ec.entity.find("moqui.device.DeviceContent").condition("deviceId", deviceId).count() + 1
    return ec.service.sync().name("create#moqui.device.DeviceContent")
        .parameters({...fields, deviceId, sequenceNum}).call().deviceContentId
}

const storeDeviceContents = (deviceId, locations, descriptionPrefix) =>
    Array.from(locations || []).filter(l => !blank(l))
        .map((location, index) => storeDeviceContent(deviceId, location, `${descriptionPrefix} #${index + 1}`))
        .filter(id => id)

// GenicamUtil.storeTensorPayload: a 3D frame as a Tensor with its axes and its NPY file as TensorContent.
function storeTensor(serial, payload, outputDir) {
    const shape = Array.from(payload.shape || []).map(size => Number(size))
    const npyBytes = payload.npy_bytes
    if (!shape.length || !npyBytes) throw new Error("The 3D frame has no shape or no NPY buffer")
    if (shape.some(size => !Number.isInteger(size) || size <= 0)) throw new Error("Tensor dimensions must be positive integers")
    const size = shape.reduce((total, dimension) => total * dimension, 1)

    const tensorId = ec.service.sync().name("create#moqui.math.Tensor").parameters({
        tensorTypeEnumId: "TtDense", purposeEnumId: "TpImageRep", name: TENSOR_NAME_PREFIX + serial,
        description: `3D depth map acquired from GenICam Camera ${serial} in format ${payload.data_format}`,
        rank: shape.length, shape: "[" + shape.join(", ") + "]", size,
    }).call().tensorId
    const axes = [["TapHeight", "Y"], ["TapWidth", "X"]]
    shape.forEach((axisSize, axisIndex) => {
        const [purposeEnumId, label] = axes[axisIndex] || ["TapChannel", "C"]
        const axisStride = shape.slice(axisIndex + 1).reduce((total, dimension) => total * dimension, 1)
        ec.service.sync().name("create#moqui.math.TensorAxis").parameters({tensorId, axisIndex, axisSize,
            axisTypeEnumId: "TatDense", axisStride, purposeEnumId, label}).call()
    })

    const contentLocation = configuredPath(outputDir, "genicam.tensors.path", DEFAULT_TENSORS_PATH) + `/tensor_${tensorId}.npy`
    ec.resource.getLocationReference("file:" + contentLocation).putBytes(npyBytes)
    const tensorContentId = ec.service.sync().name("create#moqui.math.TensorContent").parameters({
        tensorId, contentTypeEnumId: "TCntNpy", contentLocation, contentDate: ec.user.nowTimestamp,
        description: `Numpy binary for Tensor ${tensorId}`,
    }).call().tensorContentId
    return {tensorId, tensorContentId, contentLocation}
}

// ---------- Operations ----------

// run#GenICamRequest: the GenICam nodes of the request items (query) read or written; the values the camera answers
// are stored with DeviceAcquisitionServices.store#Samples, as the values of the other drivers.
function runRequest(p) {
    const {device, request, connection} = p
    if (!connection)
        throw new Error(`A connection instance is required for GenICam to communicate with the camera ${request.deviceId}. Device request with name ${request.requestName}, aborted.`)
    const items = Array.from(p.requestItemList || [])
    let results
    if (request.requestTypeEnumId === "DrtRead") {
        results = withCamera(device, connection, client => client.read(items.map(item => item.query)))
    } else if (request.requestTypeEnumId === "DrtWrite") {
        // GenicamUtil.buildParametersMap: the value of each item's Parameter, by the item's node name
        const parametersMap = {}
        for (const item of items) {
            const parameter = item.parameterId ? one("moqui.math.Parameter", "parameterId", item.parameterId) : null
            if (!parameter || blank(item.query)) continue
            const value = parameter.numericValue != null ? parameter.numericValue
                : (parameter.symbolicValue != null ? parameter.symbolicValue : parameter.parameterEnumId)
            if (value != null) parametersMap[item.query] = typeof value.toNumber === "function" ? value.toNumber() : value
        }
        results = withCamera(device, connection, client => client.write(parametersMap))
    } else {
        throw new Error(`Unsupported device request type ${request.requestTypeEnumId}. Could not send the device request with name ${request.requestName}.`)
    }
    const receivedTime = ec.user.nowTimestamp
    const samples = items.filter(item => item.parameterId && results && results[item.query] !== undefined)
        .map(item => ({deviceId: request.deviceId, parameterId: item.parameterId, value: results[item.query], receivedTime}))
    const stored = samples.length ? ec.service.sync().name("moqui.device.DeviceAcquisitionServices.store#Samples")
        .parameters({requestName: request.requestName, samples}).call() : {storedCount: 0, failedCount: 0}
    return {status: stored.failedCount ? "error" : "success", samples, storedCount: stored.storedCount}
}

function singleImage(p) {
    const result = withCamera(p.device, p.connection, client => client.acquireSingleImage(
        configuredPath(p.outputDir, "genicam.images.path", DEFAULT_IMAGES_PATH), imageFormat(p.imageFormat),
        configuredInteger(p.resizeWidth, "genicam.images.resize.width"),
        configuredInteger(p.resizeHeight, "genicam.images.resize.height")))
    return {imageLocation: result.file_path, imageFormat: result.extension, contentType: result.content_type,
        width: result.width, height: result.height,
        deviceContentId: storeDeviceContent(p.deviceId, result.file_path, "GenICam single image")}
}

function videoStream(p) {
    const result = withCamera(p.device, p.connection, client => client.acquireVideoStream(
        configuredInteger(p.numFrames, "genicam.frames.default.numFrames"),
        configuredPath(p.outputDir, "genicam.frames.path", DEFAULT_FRAMES_PATH), imageFormat(p.imageFormat),
        configuredInteger(p.resizeWidth, "genicam.frames.resize.width"),
        configuredInteger(p.resizeHeight, "genicam.frames.resize.height")))
    return {acquiredFrames: result.acquired_frames,
        deviceContentIdList: storeDeviceContents(p.deviceId, result.acquired_frames, "GenICam video stream frame")}
}

function videoFile(p) {
    const result = withCamera(p.device, p.connection, client => client.acquireVideoFile(
        configuredInteger(p.numFrames, "genicam.videos.default.numFrames"),
        configuredNumber(p.fps, "genicam.videos.default.fps"),
        configuredPath(p.outputDir, "genicam.videos.path", DEFAULT_VIDEOS_PATH),
        videoContainer(p.videoContainer), videoCodec(p.videoCodec),
        configuredInteger(p.resizeWidth, "genicam.videos.resize.width"),
        configuredInteger(p.resizeHeight, "genicam.videos.resize.height")))
    return {videoLocation: result.video_path, acquiredFrames: result.acquired_frames, fps: result.fps,
        videoContainer: result.container, videoCodec: result.codec}
}

// acquire#MultiVideoFile: each camera with its connection (the one named at the same index, else the device's).
function multiVideoFile(p) {
    const deviceIds = Array.from(p.deviceIdList || [])
    if (!deviceIds.length) throw new Error("At least one deviceId is required.")
    const connectionNames = Array.from(p.connectionNameList || [])
    const cameraList = deviceIds.map((deviceId, index) => {
        const device = one("moqui.device.Device", "deviceId", deviceId)
        if (!device) throw new Error(`Device not found with ID: ${deviceId}`)
        const connectionName = connectionNames[index]
        const connection = connectionName ? one("moqui.device.DeviceConnection", "connectionName", connectionName)
            : one("moqui.device.DeviceConnection", "deviceId", deviceId)
        if (!connection) throw new Error(`Connection not found for device: ${deviceId}`)
        checkConnection(device, connection)
        return {camera_index: index, device_id: deviceId, serial_number: serialNumber(device, connection),
            cti_path: connection.transportConfig}
    })
    const options = {
        output_dir: configuredPath(p.outputDir, "genicam.videos.path", DEFAULT_VIDEOS_PATH),
        num_frames: configuredInteger(p.numFrames, "genicam.videos.default.numFrames"),
        fps: configuredNumber(p.fps, "genicam.videos.default.fps"),
        video_container: videoContainer(p.videoContainer), video_codec: videoCodec(p.videoCodec),
        resize_width: configuredInteger(p.resizeWidth, "genicam.videos.resize.width"),
        resize_height: configuredInteger(p.resizeHeight, "genicam.videos.resize.height"),
        processTimeoutMs: configuredInteger(p.processTimeoutMs, "genicam.multi.process.timeout.ms"),
    }
    const result = ec.getTool("GenicamMultiCameraRunner").acquireMultiVideoFile(cameraList, options)
    return {captureResults: result.results, successfulCount: result.successful_count, failedCount: result.failed_count}
}

function visualServoFrame(p) {
    const result = withCamera(p.device, p.connection, client => client.acquireVisualServoFrame(
        configuredBoolean(p.useCachedFrame, "genicam.servo.default.useCachedFrame") === true,
        configuredBoolean(p.saveSnapshot, "genicam.servo.default.saveSnapshot") === true,
        configuredPath(p.outputDir, "genicam.servo.path", DEFAULT_SERVO_PATH), imageFormat(p.imageFormat),
        configuredInteger(p.resizeWidth, "genicam.servo.resize.width"),
        configuredInteger(p.resizeHeight, "genicam.servo.resize.height")))
    return {frameBytes: result.frame_bytes, jpegBytes: result.jpeg_bytes, contentType: result.content_type,
        dataFormat: result.data_format, frameFormat: result.extension, componentIndex: result.component_index,
        width: result.width, height: result.height,
        capturedAt: result.captured_at == null ? null : String(result.captured_at), source: result.source,
        snapshotLocation: result.snapshot_location, previewLocation: result.preview_location,
        snapshotDeviceContentId: storeDeviceContent(p.deviceId, result.snapshot_location, "GenICam visual servo snapshot"),
        previewDeviceContentId: storeDeviceContent(p.deviceId, result.preview_location, "GenICam visual servo preview")}
}

// stream#LiveMjpeg: the latest frame of the camera's streaming thread, maxFrames times, as a multipart response; each
// frame is flushed to the client as it is written.
function liveMjpeg(p) {
    const response = ec.web.response
    if (!response) throw new Error("This service must be called in a web request context.")
    const maxFrames = configuredInteger(p.maxFrames, "genicam.stream.max.frames") || 0
    const frameDelayMs = configuredInteger(p.frameDelayMs, "genicam.stream.frame.delay.ms") || 0
    response.setContentType("multipart/x-mixed-replace; boundary=frame")
    response.setHeader("Cache-Control", "no-cache, private, must-revalidate")
    response.setHeader("Pragma", "no-cache")
    response.setHeader("X-Accel-Buffering", "no")
    let frames = 0
    withCamera(p.device, p.connection, client => {
        for (let frameCount = 0; frameCount < maxFrames; frameCount++) {
            const jpegBytes = client.getFrameBytes()
            if (jpegBytes && jpegBytes.length) {
                response.write(`--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ${jpegBytes.length}\r\n\r\n`)
                response.write(jpegBytes)
                response.write("\r\n")
                response.flushBuffer()
                frames++
            }
            client.pause(frameDelayMs)
        }
    })
    return {sentFrames: frames}
}

function acquire3dFrame(p) {
    const serial = serialNumber(p.device, p.connection)
    const payload = withCamera(p.device, p.connection, client => client.acquire3dFrame())
    return payload ? storeTensor(serial, payload, p.outputDir) : {}
}

// clean#GenICamTensors: the NPY files of acquire#GenICam3DFrame older than daysToKeep, with their TensorContent,
// TensorAxis and Tensor; each tensor in a transaction of its own. Tensors of other components, and files outside the
// genicam.tensors.path directory, are left alone.
function cleanTensors(p) {
    const daysToKeep = Number(p.daysToKeep)
    if (!(daysToKeep >= 0)) throw new Error("daysToKeep must not be negative")
    const thresholdDate = new Date(Date.now() - daysToKeep * 24 * 60 * 60 * 1000)
    const root = configuredPath(p.outputDir, "genicam.tensors.path", DEFAULT_TENSORS_PATH).replace(/\/$/, "") + "/"
    ec.logger.info(`Cleaning up GenICam ToF tensors older than ${thresholdDate.toISOString()} (daysToKeep=${daysToKeep})...`)
    const contents = Array.from(ec.entity.find("moqui.math.TensorContent").condition("contentTypeEnumId", "TCntNpy")
        .condition("contentDate", "less-than", thresholdDate).useCache(false).list() || [])
    let deletedCount = 0
    for (const content of contents) {
        const tensor = one("moqui.math.Tensor", "tensorId", content.tensorId)
        if (!tensor || !String(tensor.name || "").startsWith(TENSOR_NAME_PREFIX)) continue
        const location = String(content.contentLocation || "")
        if (!location.startsWith(root) || location.substring(root.length).includes("..")) continue
        const file = ec.resource.getLocationReference("file:" + location)
        try {
            if (file.getExists()) {
                file.delete()
                ec.logger.info(`Deleted old tensor file: ${location}`)
            }
        } catch (error) {
            ec.logger.error(`Failed to delete tensor file ${location}: ${error}`)
            continue
        }
        try {
            ec.service.sync().name("moqui.genicam.GenicamServices.delete#GenICamTensor")
                .parameters({tensorId: content.tensorId}).requireNewTransaction(true).call()
            deletedCount++
        } catch (error) {
            ec.logger.error(`Failed to delete DB records for tensor ${content.tensorId}: ${error}`)
        }
    }
    ec.logger.info(`Successfully cleaned up ${deletedCount} old ToF tensors.`)
    return {deletedCount}
}

const operations = {runRequest, singleImage, videoStream, videoFile, multiVideoFile, visualServoFrame, liveMjpeg,
    acquire3dFrame, cleanTensors}
;({
    execute(op, parameters) {
        if (!operations[op]) throw new Error("Unknown GenICam operation " + op)
        return operations[op](parameters)
    }
})
