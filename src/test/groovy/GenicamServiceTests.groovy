package org.moqui.genicam

import spock.lang.Shared
import spock.lang.Specification
import spock.lang.Stepwise
import org.moqui.Moqui
import org.moqui.context.ExecutionContext
import org.moqui.entity.EntityValue
import org.slf4j.Logger
import org.slf4j.LoggerFactory

@Stepwise
class GenicamServiceTests extends Specification {
    @Shared protected static final Logger logger = LoggerFactory.getLogger(GenicamServiceTests)
    @Shared ExecutionContext ec

    def setupSpec() {
        // ensure database is initialized and seed/test data is loaded
        Moqui.getExecutionContextFactory().checkEmptyDb()
        ec = Moqui.getExecutionContext()
        ec.user.loginUser("john.doe", "moqui")
    }

    def cleanupSpec() {
        if (ec != null) ec.destroy()
    }

    def setup() {
        ec.artifactExecution.disableAuthz()
    }

    def cleanup() {
        ec.artifactExecution.enableAuthz()
    }

    def "test read live camera state (mock fallback)"() {
        given: "the FLIR_ReadState request exists"
            EntityValue readReq = ec.entity.find("moqui.device.DeviceRequest")
                .condition("requestName", "FLIR_ReadState").one()
            assert readReq != null

        when: "executing the read request"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_ReadState").call()

        then: "no errors are raised"
            !ec.message.hasError()

        and: "the parameters are correctly updated in the database"
            EntityValue exposure = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11001").one()
            EntityValue gain = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11002").one()
            EntityValue trigMode = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11003").one()
            EntityValue trigSrc = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11004").one()

            exposure.numericValue == 5000.0
            gain.numericValue == 0.0
            trigMode.symbolicValue == "On"
            trigSrc.symbolicValue == "Software"
            
        and: "ParameterLog entries are successfully generated"
            long logsCount = ec.entity.find("moqui.math.ParameterLog").condition("parameterId", "11001").count()
            logsCount > 0
    }

    def "test write software trigger parameter (mock fallback)"() {
        given: "the FLIR_TriggerShot request exists"
            EntityValue writeReq = ec.entity.find("moqui.device.DeviceRequest")
                .condition("requestName", "FLIR_TriggerShot").one()
            assert writeReq != null

        when: "triggering the software shot write request"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_TriggerShot").call()

        then: "no errors are raised"
            !ec.message.hasError()

        and: "the trigger parameter log indicates execution"
            EntityValue triggerParam = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11005").one()
            triggerParam.symbolicValue == "Executed"
            
            long logEntries = ec.entity.find("moqui.math.ParameterLog").condition("parameterId", "11005").count()
            logEntries > 0
    }

    def "test start and stop camera streaming (mock fallback)"() {
        given: "the streaming requests exist"
            EntityValue startReq = ec.entity.find("moqui.device.DeviceRequest")
                .condition("requestName", "FLIR_StartStreaming").one()
            EntityValue stopReq = ec.entity.find("moqui.device.DeviceRequest")
                .condition("requestName", "FLIR_StopStreaming").one()
            assert startReq != null && stopReq != null

        when: "triggering the start streaming request"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_StartStreaming").call()

        then: "no errors are raised"
            !ec.message.hasError()

        when: "triggering the stop streaming request"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_StopStreaming").call()

        then: "no errors are raised"
            !ec.message.hasError()
    }

    def "test read latest frame (mock fallback)"() {
        given: "starting the background stream"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_StartStreaming").call()
            assert !ec.message.hasError()

        when: "fetching the latest frame"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_GetFrame").call()

        then: "no errors are raised"
            !ec.message.hasError()

        and: "the LatestFrame parameter is updated with a valid file path"
            EntityValue latestFrameParam = ec.entity.find("moqui.math.Parameter").condition("parameterId", "11009").one()
            String filePath = latestFrameParam.symbolicValue
            assert filePath != null && filePath.endsWith(".jpg")

        and: "the saved JPEG image exists on the filesystem and is non-empty"
            File imageFile = new File(filePath)
            assert imageFile.exists() && imageFile.length() > 0

        cleanup: "stopping the stream to clean up the acquisition thread"
            ec.service.sync().name("moqui.device.DeviceServices.run#DeviceRequest")
                .parameter("requestName", "FLIR_StopStreaming").call()
    }
}
